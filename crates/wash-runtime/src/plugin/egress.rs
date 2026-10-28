//! Egress policy supplied to native host plugins.

use std::sync::Arc;

use anyhow::{Context as _, anyhow, bail};

use crate::host::allowed_hosts::AllowedHost;
use crate::host::allowed_ip_name::{AllowedIpName, check_allowed_lookup, lookup_denial};
use crate::host::allowed_loopback::{AllowedLoopbackPort, check_allowed_loopback};
use crate::host::declared_port::Protocol;
use crate::host::egress_policy::EgressAddressPolicy;
use crate::host::ports::PortTable;
use crate::host::quota::PolicyMeters;
use crate::sockets::DenyReason;
use crate::sockets::policy::{EgressMode, SocketPolicy};

/// Schemes one client reads as the same protocol, so an `allowedHosts` entry
/// spelled either way permits an endpoint spelled the other.
const SCHEME_ALIASES: &[&[&str]] = &[
    &["postgres", "postgresql"],
    &["redis", "valkey"],
    &["rediss", "valkeys"],
];

/// A TLS scheme and the plaintext scheme it upgrades. An entry granting the
/// plaintext scheme also grants TLS to the same endpoint; never the reverse.
const TLS_UPGRADES: &[(&str, &str)] = &[("rediss", "redis"), ("tls", "nats")];

/// `scheme` and every spelling [`SCHEME_ALIASES`] groups with it.
fn aliases_of(scheme: &str) -> impl Iterator<Item = &str> {
    let group = SCHEME_ALIASES
        .iter()
        .find(|group| group.iter().any(|s| s.eq_ignore_ascii_case(scheme)));
    std::iter::once(scheme).chain(group.into_iter().flat_map(|group| group.iter().copied()))
}

/// Every entry scheme that admits an endpoint dialed with `scheme`: its own
/// spellings, and for a TLS scheme the plaintext scheme it upgrades.
fn admitting_schemes(scheme: &str) -> Vec<&str> {
    let mut schemes: Vec<&str> = aliases_of(scheme).collect();
    let upgraded = TLS_UPGRADES
        .iter()
        .find(|(tls, _)| schemes.iter().any(|s| s.eq_ignore_ascii_case(tls)));
    if let Some((_, plaintext)) = upgraded {
        schemes.extend(aliases_of(plaintext));
    }
    schemes.sort_unstable_by_key(|s| s.to_ascii_lowercase());
    schemes.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    schemes
}

/// Whether `allowed` admits an endpoint dialed with `scheme`: an entry that
/// names a scheme must name one of [`admitting_schemes`], so
/// `rediss://10.0.0.5` does not admit plaintext `redis://10.0.0.5`.
fn scheme_permits(allowed: &AllowedHost, admitting: &[&str]) -> bool {
    match allowed {
        AllowedHost::Url(url) => admitting
            .iter()
            .any(|s| s.eq_ignore_ascii_case(url.scheme())),
        _ => true,
    }
}

/// `server` with any userinfo replaced, so an error naming a malformed NATS
/// url does not carry its credentials.
fn redact_userinfo(server: &str) -> String {
    let (prefix, rest) = match server.find("://") {
        Some(i) => server.split_at(i + 3),
        None => ("", server),
    };
    // The last `@` anywhere: a raw password may hold `/`, `?` or `#`, which
    // would end the authority early.
    match rest.rfind('@') {
        Some(at) => format!("{prefix}***@{}", &rest[at + 1..]),
        None => server.to_string(),
    }
}

/// A native plugin's declared network ceiling.
///
/// Native plugins do not pass through WASI socket hooks. Each implementation
/// must check this policy before its networking library connects. This checks
/// the declared endpoint; a native client's own DNS and socket operations
/// cannot be intercepted for resolved-address filtering.
///
/// The rules are the ones [`SocketPolicy`] applies to a workload's sockets, so
/// the same declaration means the same thing on both sides: the loopback and
/// host-owned-port refusals hold whatever the mode, while an `allowedHosts` or
/// range refusal is counted rather than enforced under [`EgressMode::Count`].
/// An entry's port is [`AllowedHost::client_port`], for an endpoint naming an
/// address and one naming a host alike.
#[derive(Debug, Clone)]
pub struct PluginEgressPolicy {
    allowed_hosts: Arc<[AllowedHost]>,
    allowed_ip_name_lookups: Arc<[AllowedIpName]>,
    allowed_host_loopback_ports: Arc<[AllowedLoopbackPort]>,
    host_loopback_enabled: bool,
    host_owned_ports: Option<Arc<PortTable>>,
    egress_addrs: EgressAddressPolicy,
    egress_mode: EgressMode,
    meters: Option<Arc<PolicyMeters>>,
}

impl PluginEgressPolicy {
    /// The operator's three lists, under the host-wide policy that gates them.
    ///
    /// Everything but the lists is read from `socket_policy`, so a native
    /// plugin cannot end up on a different gate, range policy, or mode than the
    /// workloads beside it.
    pub(crate) fn new(
        allowed_hosts: Arc<[AllowedHost]>,
        allowed_ip_name_lookups: Arc<[AllowedIpName]>,
        allowed_host_loopback_ports: Arc<[AllowedLoopbackPort]>,
        socket_policy: &SocketPolicy,
    ) -> Self {
        Self {
            allowed_hosts,
            allowed_ip_name_lookups,
            allowed_host_loopback_ports,
            host_loopback_enabled: socket_policy.host_loopback_enabled,
            host_owned_ports: socket_policy.host_owned_ports.clone(),
            egress_addrs: socket_policy.egress_addrs,
            egress_mode: socket_policy.egress_mode,
            meters: socket_policy.meters.clone(),
        }
    }

    /// Whether two policies carry the same operator declaration.
    ///
    /// Only the declared lists and the host-wide loopback switch are compared:
    /// the rest comes from the host's own socket policy, which is one value
    /// both sides read.
    #[cfg(feature = "host-component-plugins")]
    pub(crate) fn same_declaration(&self, other: &Self) -> bool {
        self.allowed_hosts == other.allowed_hosts
            && self.allowed_ip_name_lookups == other.allowed_ip_name_lookups
            && self.allowed_host_loopback_ports == other.allowed_host_loopback_ports
            && self.host_loopback_enabled == other.host_loopback_enabled
    }

    /// Whether loopback ports were declared while the host-wide gate is off.
    #[must_use]
    pub fn disabled_loopback_grants(&self) -> bool {
        !self.host_loopback_enabled && !self.allowed_host_loopback_ports.is_empty()
    }

    /// Checks a URL before a native client connects.
    ///
    /// `default_port` is used when the URL omits one. A domain endpoint must
    /// be permitted by the host list, which also lets it be resolved.
    /// Loopback endpoints use the separate host-loopback grant and the
    /// host-wide enable switch.
    pub fn check_url(
        &self,
        target: &str,
        protocol: Protocol,
        default_port: u16,
    ) -> anyhow::Result<()> {
        let url = url::Url::parse(target)
            .with_context(|| format!("native plugin endpoint {target:?} is not a valid URL"))?;
        let host = url
            .host_str()
            .with_context(|| format!("native plugin endpoint {target:?} has no host"))?;
        let port = url.port_or_known_default().unwrap_or(default_port);
        let parsed_host = url::Host::parse(host)?;
        let literal_ip = match &parsed_host {
            url::Host::Ipv4(addr) => Some(core::net::IpAddr::V4(*addr)),
            url::Host::Ipv6(addr) => Some(core::net::IpAddr::V6(*addr)),
            url::Host::Domain(_) => None,
        };

        // `to_canonical` first, so an IPv4-mapped literal such as
        // `::ffff:127.0.0.1` is the loopback it addresses rather than an
        // ordinary IPv6 destination. A native client dialing `0.0.0.0` or `::`
        // reaches this machine too, so the unspecified address is loopback.
        let loopback = literal_ip.is_some_and(|addr| {
            let addr = addr.to_canonical();
            addr.is_loopback() || addr.is_unspecified()
        }) || host.trim_end_matches('.').eq_ignore_ascii_case("localhost");
        if loopback {
            if !self.host_loopback_enabled {
                bail!(
                    "native plugin endpoint {target:?} reaches host loopback, but host-loopback \
                     access is disabled"
                );
            }
            let addr =
                core::net::SocketAddr::new(core::net::IpAddr::V4([127, 0, 0, 1].into()), port);
            if !check_allowed_loopback(&self.allowed_host_loopback_ports, addr, protocol) {
                bail!(
                    "native plugin endpoint {target:?} reaches host loopback port {port}/{protocol}, \
                     which allowedHostLoopbackPorts does not permit"
                );
            }
            if let Some(table) = &self.host_owned_ports {
                let ipv4 = core::net::SocketAddr::new(core::net::Ipv4Addr::LOCALHOST.into(), port);
                let ipv6 = core::net::SocketAddr::new(core::net::Ipv6Addr::LOCALHOST.into(), port);
                let host_owned = match literal_ip.map(|ip| ip.to_canonical()) {
                    Some(core::net::IpAddr::V4(_)) => table.is_published(protocol, ipv4),
                    Some(core::net::IpAddr::V6(_)) => table.is_published(protocol, ipv6),
                    None => {
                        table.is_published(protocol, ipv4) || table.is_published(protocol, ipv6)
                    }
                };
                if host_owned {
                    bail!(
                        "native plugin endpoint {target:?} reaches host-owned port {port}/{protocol}"
                    );
                }
            }
            return Ok(());
        }

        // An address carries no name, so it answers to `permits_addr` — the
        // matcher `SocketPolicy` uses — and never to a suffix wildcard, which
        // describes names only. A name is matched as a name, and cannot be
        // range-checked here at all: the native client resolves it itself,
        // outside anything this policy sees.
        let admitting = admitting_schemes(url.scheme());
        if let Some(ip) = literal_ip {
            let addr = core::net::SocketAddr::new(ip, port);
            // Both judged, as `SocketPolicy::resolve_connect` does: counting
            // records every refusal, enforcing returns the first.
            let permitted = self.allowed_hosts.iter().any(|allowed| {
                allowed.permits_client_addr(addr) && scheme_permits(allowed, &admitting)
            });
            if !permitted {
                self.gate(
                    DenyReason::NotPermitted,
                    format!("native plugin endpoint {target:?} is not permitted by allowedHosts"),
                )?;
            }
            if !self.egress_addrs.permits(ip) {
                self.gate(
                    DenyReason::BlockedRange,
                    format!(
                        "native plugin endpoint {target:?} is in an address range the egress \
                         policy denies"
                    ),
                )?;
            }
            return Ok(());
        }

        // As for a workload: a name `allowedHosts` permits connecting to may
        // be resolved, and `allowedIpNameLookups` adds names beyond those.
        if !check_allowed_lookup(
            &self.allowed_ip_name_lookups,
            &self.allowed_hosts,
            &parsed_host,
        ) {
            bail!(
                "native plugin endpoint {target:?}: {}",
                lookup_denial(&self.allowed_hosts, &parsed_host)
            );
        }
        let uris = admitting
            .iter()
            .map(|scheme| format!("{scheme}://{host}:{port}").parse::<http::Uri>())
            .collect::<Result<Vec<_>, _>>()
            .with_context(|| format!("native plugin endpoint {target:?} is not a valid URI"))?;
        // A plugin has a scheme, so an entry carrying one must agree with it —
        // which `matches` checks — and a port-less entry means the client's
        // default port.
        let name = crate::sockets::resolved_names::normalize_name(&parsed_host);
        let permitted = |allowed: &AllowedHost| match allowed {
            AllowedHost::Any => true,
            _ => {
                uris.iter().any(|uri| allowed.matches(uri))
                    && name
                        .as_deref()
                        .is_some_and(|name| allowed.permits_client_name(name, port))
            }
        };
        if !self.allowed_hosts.iter().any(permitted) {
            return self.gate(
                DenyReason::NotPermitted,
                format!("native plugin endpoint {target:?} is not permitted by allowedHosts"),
            );
        }
        Ok(())
    }

    /// Checks one NATS server address as `async_nats` reads it: a schemeless
    /// address is `nats://`, and a missing port is 4222.
    ///
    /// Checked as `scheme://host:port` rebuilt from the parsed address, so a
    /// refusal never repeats the url's credentials.
    pub fn check_nats_server(&self, server: &str) -> anyhow::Result<()> {
        let addr = server
            .parse::<async_nats::ServerAddr>()
            .with_context(|| format!("NATS server {:?} is invalid", redact_userinfo(server)))?;
        self.check_host_port(addr.scheme(), addr.host(), addr.port())
            .context("NATS server is denied")
    }

    /// Checks a TCP `host` and `port` a native client takes from something
    /// other than a URL, such as a parsed postgres or redis connection config.
    pub fn check_host_port(&self, scheme: &str, host: &str, port: u16) -> anyhow::Result<()> {
        let target = match host.parse::<core::net::Ipv6Addr>() {
            Ok(_) => format!("{scheme}://[{host}]:{port}"),
            Err(_) => format!("{scheme}://{host}:{port}"),
        };
        self.check_url(&target, Protocol::Tcp, port)
    }

    /// An enforcing policy permitting exactly `hosts`, for tests of the
    /// plugins that check it.
    #[cfg(test)]
    pub(crate) fn allowing(hosts: &[&str]) -> Arc<Self> {
        Arc::new(Self::new(
            hosts.iter().map(|host| host.parse().unwrap()).collect(),
            Arc::from([]),
            Arc::from([]),
            &SocketPolicy {
                egress_mode: EgressMode::Enforce,
                ..Default::default()
            },
        ))
    }

    /// Apply a refusal under the host's [`EgressMode`], the counterpart of
    /// [`SocketPolicy::gate`]: refuse it, or count it and allow it so an
    /// operator sees the blast radius without severing live traffic.
    fn gate(&self, reason: DenyReason, message: String) -> anyhow::Result<()> {
        match self.egress_mode {
            EgressMode::Enforce => Err(anyhow!(message)),
            EgressMode::Count => {
                if let Some(meters) = &self.meters {
                    meters.record_would_deny(reason);
                }
                tracing::debug!(
                    reason = reason.as_str(),
                    "{message}; allowing it because the host is in count mode"
                );
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host enforcing its gate, which is what every test here but the
    /// count-mode one wants: a refusal reads as an error.
    fn enforcing(host_loopback_enabled: bool) -> SocketPolicy {
        SocketPolicy {
            host_loopback_enabled,
            egress_mode: EgressMode::Enforce,
            ..Default::default()
        }
    }

    fn policy(enabled: bool) -> PluginEgressPolicy {
        PluginEgressPolicy::new(
            Arc::from(["nats://example.com:4222".parse().unwrap()]),
            Arc::from(["example.com".parse().unwrap()]),
            Arc::from([AllowedLoopbackPort::tcp(4222)]),
            &enforcing(enabled),
        )
    }

    #[test]
    fn external_names_require_both_grants() {
        policy(false)
            .check_url("nats://example.com:4222", Protocol::Tcp, 4222)
            .unwrap();
        assert!(
            policy(false)
                .check_url("nats://other.example:4222", Protocol::Tcp, 4222)
                .is_err()
        );
    }

    /// An entry naming a scheme holds an address endpoint to it, as it holds a
    /// named one: TLS granted is not plaintext granted.
    #[test]
    fn a_literal_address_entry_holds_its_scheme() {
        let egress = PluginEgressPolicy::allowing(&["rediss://10.0.0.5:6379", "10.0.0.6:6379"]);
        egress.check_host_port("rediss", "10.0.0.5", 6379).unwrap();
        assert!(egress.check_host_port("redis", "10.0.0.5", 6379).is_err());
        // An entry with no scheme admits any.
        egress.check_host_port("redis", "10.0.0.6", 6379).unwrap();
    }

    /// A plaintext entry admits TLS to the same endpoint, by name or address;
    /// a TLS entry never admits plaintext.
    #[test]
    fn a_plaintext_entry_admits_its_tls_upgrade() {
        let egress = PluginEgressPolicy::allowing(&[
            "nats://nats.internal:4222",
            "redis://10.0.0.5:6379",
            "tls://secure.internal:4222",
        ]);
        egress
            .check_nats_server("tls://nats.internal:4222")
            .unwrap();
        egress.check_host_port("rediss", "10.0.0.5", 6379).unwrap();
        egress.check_host_port("valkeys", "10.0.0.5", 6379).unwrap();
        egress.check_nats_server("tls://secure.internal").unwrap();
        assert!(
            egress
                .check_nats_server("nats://secure.internal:4222")
                .is_err()
        );
    }

    /// A refusal names the endpoint without the credentials in its url.
    #[test]
    fn a_nats_refusal_does_not_repeat_credentials() {
        let egress = PluginEgressPolicy::allowing(&["nats://nats.internal:4222"]);
        for server in [
            "nats://svc:S3cret@evil.example:4222",
            "svc:S3cret@evil.example",
            "nats://svc:S3cret@evil.example:42x2",
            "nats://svc:S3/cr#et@evil.example:42x2",
        ] {
            let err = format!("{:#}", egress.check_nats_server(server).unwrap_err());
            assert!(!err.contains("S3cret"), "{err}");
            assert!(err.contains("evil.example"), "{err}");
        }
    }

    /// `0.0.0.0` and `::` reach this machine, so they need the loopback grant.
    #[test]
    fn the_unspecified_address_is_loopback() {
        let permissive = SocketPolicy {
            egress_mode: EgressMode::Count,
            egress_addrs: crate::host::egress_policy::EgressAddressPolicy::permissive(),
            ..Default::default()
        };
        let egress = PluginEgressPolicy::new(
            Arc::from(["*".parse().unwrap()]),
            Arc::from([]),
            Arc::from([]),
            &permissive,
        );
        for target in ["redis://0.0.0.0:6379", "redis://[::]:6379"] {
            let err = egress.check_url(target, Protocol::Tcp, 6379).unwrap_err();
            assert!(
                format!("{err:#}").contains("host loopback"),
                "{target}: {err:#}"
            );
        }
    }

    #[test]
    fn loopback_requires_the_host_switch_and_port() {
        assert!(
            policy(false)
                .check_url("nats://localhost:4222", Protocol::Tcp, 4222)
                .is_err()
        );
        policy(true)
            .check_url("nats://127.0.0.1:4222", Protocol::Tcp, 4222)
            .unwrap();
        assert!(
            policy(true)
                .check_url("nats://localhost:4223", Protocol::Tcp, 4222)
                .is_err()
        );
    }

    #[test]
    fn url_scheme_defaults_are_checked_at_the_real_port() {
        let policy = PluginEgressPolicy::new(
            Arc::from(["wss://example.com:443".parse().unwrap()]),
            Arc::from(["example.com".parse().unwrap()]),
            Arc::from([]),
            &enforcing(false),
        );
        policy
            .check_url("wss://example.com", Protocol::Tcp, 4222)
            .unwrap();
    }

    #[test]
    fn native_loopback_cannot_reach_a_host_owned_port() {
        let table = PortTable::new();
        let _reservation = table
            .reserve(
                Protocol::Tcp,
                "127.0.0.1:4222".parse().unwrap(),
                crate::host::ports::PortOwner::Host("test".into()),
            )
            .unwrap();
        let policy = PluginEgressPolicy::new(
            Arc::from([]),
            Arc::from([]),
            Arc::from([AllowedLoopbackPort::tcp(4222)]),
            &SocketPolicy {
                host_loopback_enabled: true,
                egress_mode: EgressMode::Enforce,
                host_owned_ports: Some(table),
                ..Default::default()
            },
        );
        assert!(
            policy
                .check_url("nats://localhost:4222", Protocol::Tcp, 4222)
                .is_err()
        );
    }

    /// An IPv4-mapped literal addresses the machine's loopback, so it answers
    /// to the loopback grant rather than slipping past it into `allowedHosts`.
    #[test]
    fn a_mapped_ipv4_loopback_is_loopback() {
        let policy = PluginEgressPolicy::new(
            Arc::from(["*".parse().unwrap()]),
            Arc::from([]),
            Arc::from([]),
            &enforcing(true),
        );
        assert!(
            policy
                .check_url("nats://[::ffff:127.0.0.1]:4222", Protocol::Tcp, 4222)
                .is_err()
        );
    }

    /// A suffix describes names, and an address has none — the rule
    /// `AllowedHost::permits_addr` states and `SocketPolicy` applies.
    #[test]
    fn a_suffix_wildcard_never_authorises_an_address() {
        let policy = PluginEgressPolicy::new(
            Arc::from(["*.1".parse().unwrap()]),
            Arc::from([]),
            Arc::from([]),
            &enforcing(false),
        );
        assert!(
            policy
                .check_url("nats://10.0.0.1:4222", Protocol::Tcp, 4222)
                .is_err()
        );
    }

    /// The range policy reaches a native plugin's literal endpoints too, so
    /// `allowedHosts: ["*"]` does not hand it the link-local metadata service.
    #[test]
    fn a_special_range_is_denied_to_a_native_plugin() {
        let policy = PluginEgressPolicy::new(
            Arc::from(["*".parse().unwrap()]),
            Arc::from([]),
            Arc::from([]),
            &enforcing(false),
        );
        assert!(
            policy
                .check_url("http://169.254.169.254:80", Protocol::Tcp, 80)
                .is_err()
        );
    }

    /// Count mode is how an operator sees the blast radius without enforcing,
    /// so a native plugin measures it the same way a workload's sockets do.
    #[test]
    fn count_mode_counts_an_undeclared_endpoint_instead_of_refusing_it() {
        let meters = Arc::new(PolicyMeters::default());
        let policy = PluginEgressPolicy::new(
            Arc::from([]),
            Arc::from(["other.example".parse().unwrap()]),
            Arc::from([]),
            &SocketPolicy {
                egress_mode: EgressMode::Count,
                meters: Some(Arc::clone(&meters)),
                ..Default::default()
            },
        );
        policy
            .check_url("nats://other.example:4222", Protocol::Tcp, 4222)
            .unwrap();
        assert_eq!(meters.would_deny(DenyReason::NotPermitted), 1);
        // Loopback is refused whatever the mode: reaching the machine itself
        // takes a grant, and counting it would hand out the access the
        // host-wide switch exists to withhold.
        assert!(
            policy
                .check_url("nats://127.0.0.1:4222", Protocol::Tcp, 4222)
                .is_err()
        );
    }

    /// A native plugin's endpoint is held to the port a workload's socket to
    /// the same destination would be: a scheme with no port means that
    /// scheme's default, whether the entry names an address or a host.
    #[test]
    fn an_entry_with_a_scheme_and_no_port_permits_only_the_default_port() {
        let egress = PluginEgressPolicy::new(
            [
                "https://vault.internal",
                "https://10.0.0.5",
                "nats://nats.internal",
            ]
            .iter()
            .map(|entry| entry.parse().unwrap())
            .collect(),
            Arc::from(["*".parse().unwrap()]),
            Arc::from([]),
            &enforcing(false),
        );
        let check = |target| egress.check_url(target, Protocol::Tcp, 4222);

        for target in ["https://vault.internal", "https://10.0.0.5"] {
            check(target).unwrap();
        }
        for target in ["https://vault.internal:8200", "https://10.0.0.5:8200"] {
            assert!(check(target).is_err(), "{target}");
        }
        // The scheme still has to agree, and a client scheme means its
        // client's default port.
        assert!(check("http://vault.internal:443").is_err());
        check("nats://nats.internal:4222").unwrap();
        assert!(check("nats://nats.internal:6222").is_err());
    }

    /// A native plugin's endpoint named in `allowedHosts` needs no second
    /// entry to be resolved, exactly as for a workload's socket.
    #[test]
    fn a_named_endpoint_needs_no_separate_lookup_grant() {
        let egress = PluginEgressPolicy::new(
            Arc::from(["nats://nats.internal:4222".parse().unwrap()]),
            Arc::from([]),
            Arc::from([]),
            &enforcing(false),
        );
        egress
            .check_url("nats://nats.internal:4222", Protocol::Tcp, 4222)
            .unwrap();
        assert!(
            egress
                .check_url("nats://other.internal:4222", Protocol::Tcp, 4222)
                .is_err()
        );

        // `*` permits the connection but names nothing to resolve.
        let any = PluginEgressPolicy::new(
            Arc::from(["*".parse().unwrap()]),
            Arc::from([]),
            Arc::from([]),
            &enforcing(false),
        );
        assert!(
            any.check_url("nats://nats.internal:4222", Protocol::Tcp, 4222)
                .is_err()
        );
    }

    /// The native counterpart of the socket policy's count-mode test: an
    /// endpoint that fails both layers is counted under both.
    #[test]
    fn count_mode_records_every_reason_an_endpoint_would_be_refused() {
        let meters = Arc::new(PolicyMeters::default());
        let policy = PluginEgressPolicy::new(
            Arc::from([]),
            Arc::from([]),
            Arc::from([]),
            &SocketPolicy {
                egress_mode: EgressMode::Count,
                meters: Some(Arc::clone(&meters)),
                ..Default::default()
            },
        );
        policy
            .check_url("http://169.254.169.254", Protocol::Tcp, 80)
            .unwrap();
        assert_eq!(meters.would_deny(DenyReason::NotPermitted), 1);
        assert_eq!(meters.would_deny(DenyReason::BlockedRange), 1);
    }
}
