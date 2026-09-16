//! Egress policy supplied to native host plugins.

use std::sync::Arc;

use anyhow::{Context as _, anyhow, bail};

use crate::host::allowed_hosts::{AllowedHost, check_allowed_addr};
use crate::host::allowed_ip_name::{AllowedIpName, check_allowed_ip_name};
use crate::host::allowed_loopback::{AllowedLoopbackPort, check_allowed_loopback};
use crate::host::declared_port::Protocol;
use crate::host::egress_policy::EgressAddressPolicy;
use crate::host::ports::PortTable;
use crate::host::quota::PolicyMeters;
use crate::sockets::DenyReason;
use crate::sockets::policy::{EgressMode, SocketPolicy};

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

    /// Whether loopback ports were declared while the host-wide gate is off.
    #[must_use]
    pub fn disabled_loopback_grants(&self) -> bool {
        !self.host_loopback_enabled && !self.allowed_host_loopback_ports.is_empty()
    }

    /// Checks a URL before a native client connects.
    ///
    /// `default_port` is used when the URL omits one. Domain endpoints must
    /// satisfy both the host and name-lookup lists. Loopback endpoints use the
    /// separate host-loopback grant and the host-wide enable switch.
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
        // ordinary IPv6 destination — the same normalization
        // `SocketPolicy::resolve_connect` applies.
        let loopback = literal_ip.is_some_and(|addr| addr.to_canonical().is_loopback())
            || host.trim_end_matches('.').eq_ignore_ascii_case("localhost");
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
        if let Some(ip) = literal_ip {
            let addr = core::net::SocketAddr::new(ip, port);
            if !check_allowed_addr(&self.allowed_hosts, addr) {
                return self.gate(
                    DenyReason::NotPermitted,
                    format!("native plugin endpoint {target:?} is not permitted by allowedHosts"),
                );
            }
            if !self.egress_addrs.permits(ip) {
                return self.gate(
                    DenyReason::BlockedRange,
                    format!(
                        "native plugin endpoint {target:?} is in an address range the egress \
                         policy denies"
                    ),
                );
            }
            return Ok(());
        }

        if !check_allowed_ip_name(&self.allowed_ip_name_lookups, &parsed_host) {
            bail!(
                "native plugin endpoint {target:?} uses a name which \
                 allowedIpNameLookups does not permit"
            );
        }
        let uri: http::Uri = format!("{}://{host}:{port}", url.scheme())
            .parse()
            .with_context(|| format!("native plugin endpoint {target:?} is not a valid URI"))?;
        if !self
            .allowed_hosts
            .iter()
            .any(|allowed| allowed.matches(&uri))
        {
            return self.gate(
                DenyReason::NotPermitted,
                format!("native plugin endpoint {target:?} is not permitted by allowedHosts"),
            );
        }
        Ok(())
    }

    /// Apply a refusal under the host's [`EgressMode`], the counterpart of
    /// [`SocketPolicy::gate`]: refuse it, or count it and allow it so an
    /// operator sees the blast radius before enforcement severs live traffic.
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

    /// Count mode is how an operator sees the blast radius before enforcing,
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
}
