//! Address-range policy for outbound connections.
//!
//! This is the second of two layers gating egress. Layer 1 is the guest's
//! declared `allowedHosts`, the same matcher `wasi:http` uses — that is what
//! actually closes the hole, because a range policy alone still lets a guest
//! dial the Kubernetes API on an ordinary routable address. Layer 2 is this:
//! applied to every address layer 1 allowed, *including whatever DNS returned
//! for a permitted name*.
//!
//! That second part is the point. A name under an attacker's control resolving
//! to `127.0.0.1` or `169.254.169.254` passes an `allowedHosts` check that only
//! ever saw the name. Re-checking the resolved address is the standard defense
//! and it is what makes an allowlist of names safe to write.
//!
//! Private ranges are permitted by default: in-cluster service traffic is the
//! common case, and denying it would make the policy unusable for the
//! deployments that need it most.

use core::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use crate::host::declared_port::Protocol;
use crate::host::ports::PortTable;
use crate::host::quota::PolicyMeters;
use crate::sockets::DenyReason;
use crate::sockets::policy::{EgressMode, SocketPolicy};

/// Which address ranges an owner may reach once its `allowedHosts` permitted
/// the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EgressAddressPolicy {
    /// Deny loopback, link-local (including the cloud metadata address),
    /// unspecified, multicast, and documentation ranges — in every spelling,
    /// including IPv4-mapped IPv6.
    pub deny_special: bool,
    /// Permit RFC1918 / ULA / carrier-grade NAT ranges. On by default: reaching
    /// a sibling service on a private address is the ordinary case.
    pub allow_private: bool,
}

impl Default for EgressAddressPolicy {
    fn default() -> Self {
        Self {
            deny_special: true,
            allow_private: true,
        }
    }
}

impl EgressAddressPolicy {
    /// A policy that permits everything, for hosts that have not opted into
    /// range filtering.
    pub fn permissive() -> Self {
        Self {
            deny_special: false,
            allow_private: true,
        }
    }

    /// Whether `addr` may be dialed.
    ///
    /// Evaluated against the canonical form, so an IPv4-mapped IPv6 address is
    /// judged as the IPv4 address it is.
    #[must_use]
    pub fn permits(&self, addr: IpAddr) -> bool {
        let addr = addr.to_canonical();
        if self.deny_special && is_special(addr) {
            return false;
        }
        if !self.allow_private && is_private(addr) {
            return false;
        }
        true
    }
}

/// The address half of the egress gate, for callers that resolve a name
/// themselves rather than going through the socket hooks: `wasi:http`.
///
/// Layer 1, the declared `allowedHosts`, has already judged the *name* by the
/// time this runs. This judges each address the name resolved to, with the
/// same range policy, host port table, mode, and counters as the host's
/// [`SocketPolicy`], which is the only place it is built from.
///
/// [`SocketPolicy`]: crate::sockets::policy::SocketPolicy
#[derive(Debug, Clone)]
pub struct EgressAddressGate {
    egress_addrs: EgressAddressPolicy,
    egress_mode: EgressMode,
    host_owned_ports: Option<Arc<PortTable>>,
    meters: Option<Arc<PolicyMeters>>,
}

/// Why an address was refused, carried to the transport's error so it can be
/// told apart from a connection refused or a spent quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EgressRefused(pub DenyReason);

impl core::fmt::Display for EgressRefused {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "egress address policy refused the destination ({})",
            self.0.as_str()
        )
    }
}

impl std::error::Error for EgressRefused {}

impl EgressAddressGate {
    /// The host's socket policy, as it applies to an address rather than a
    /// socket.
    pub fn from_socket_policy(policy: &SocketPolicy) -> Self {
        Self {
            egress_addrs: policy.egress_addrs,
            egress_mode: policy.egress_mode,
            host_owned_ports: policy.host_owned_ports.clone(),
            meters: policy.meters.clone(),
        }
    }

    /// The candidates the connection may be made to, in the order given.
    ///
    /// Under [`EgressMode::Enforce`] a refused candidate is dropped, and when
    /// none survive the first refusal is returned. Under [`EgressMode::Count`]
    /// every candidate is kept and each refusal is counted.
    pub fn filter(
        &self,
        candidates: impl IntoIterator<Item = SocketAddr>,
    ) -> Result<Vec<SocketAddr>, EgressRefused> {
        let mut permitted = Vec::new();
        let mut refused = None;
        for addr in candidates {
            let Err(reason) = self.judge(addr) else {
                permitted.push(addr);
                continue;
            };
            match self.egress_mode {
                EgressMode::Enforce => {
                    if let Some(meters) = &self.meters {
                        meters.record_deny(reason);
                    }
                    tracing::warn!(
                        %addr,
                        reason = reason.as_str(),
                        "HTTP egress address policy refused a resolved address"
                    );
                    refused.get_or_insert(reason);
                }
                EgressMode::Count => {
                    if let Some(meters) = &self.meters {
                        meters.record_would_deny(reason);
                    }
                    tracing::debug!(
                        %addr,
                        reason = reason.as_str(),
                        "HTTP egress address policy would refuse a resolved address; allowing it \
                         because the host is in count mode"
                    );
                    permitted.push(addr);
                }
            }
        }
        match (permitted.is_empty(), refused) {
            (true, Some(reason)) => Err(EgressRefused(reason)),
            _ => Ok(permitted),
        }
    }

    /// One address, before the mode is applied.
    fn judge(&self, addr: SocketAddr) -> Result<(), DenyReason> {
        if !self.egress_addrs.permits(addr.ip()) {
            return Err(DenyReason::BlockedRange);
        }
        // Reached only when the range policy lets the machine's own addresses
        // through. A loopback or unspecified spelling reaches whatever the host
        // bound on loopback, so it is judged as the loopback address the
        // host's own reservations are keyed on.
        if let Some(table) = &self.host_owned_ports {
            let ip = addr.ip().to_canonical();
            let owned =
                |ip: IpAddr| table.is_published(Protocol::Tcp, SocketAddr::new(ip, addr.port()));
            let local = ip.is_loopback() || ip.is_unspecified();
            if owned(ip)
                || (local
                    && (owned(Ipv4Addr::LOCALHOST.into()) || owned(Ipv6Addr::LOCALHOST.into())))
            {
                return Err(DenyReason::HostOwnedPort);
            }
        }
        Ok(())
    }
}

/// Ranges that are never an ordinary egress destination: the machine itself,
/// its link, its metadata service, and addresses that are not unicast.
fn is_special(addr: IpAddr) -> bool {
    // The IPv6 metadata address sits inside the unique-local range, so
    // `allow_private` would otherwise wave it through. It is the single most
    // valuable thing a guest reaches by resolving a name it controls, so it is
    // denied on its own account rather than as part of a range.
    if addr == IpAddr::V6(METADATA_V6) {
        return true;
    }
    match addr {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
                || is_v4_documentation(v4)
                // 240.0.0.0/4
                || v4.octets()[0] >= 240
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fe80::/10 link-local
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // 2001:db8::/32 documentation
                || (v6.segments()[0] == 0x2001 && v6.segments()[1] == 0x0db8)
        }
    }
}

fn is_v4_documentation(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    matches!(
        (o[0], o[1], o[2]),
        (192, 0, 2) | (198, 51, 100) | (203, 0, 113)
    )
}

fn is_private(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            v4.is_private()
                // 100.64.0.0/10 carrier-grade NAT
                || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
        }
        // fc00::/7 unique local
        IpAddr::V6(v6) => (v6.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// The cloud instance-metadata address, called out because it is the single
/// most valuable target a guest can reach by resolving a name it controls.
pub const METADATA_V4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);

/// The IPv6 instance-metadata address used by the major clouds.
pub const METADATA_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254);

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("test address should parse")
    }

    #[test]
    fn the_default_denies_the_machine_and_its_link() {
        let policy = EgressAddressPolicy::default();
        for denied in [
            "127.0.0.1",
            "127.255.255.254",
            "0.0.0.0",
            "169.254.169.254",
            "169.254.1.1",
            "224.0.0.1",
            "255.255.255.255",
            "192.0.2.10",
            "203.0.113.10",
            "240.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
        ] {
            assert!(!policy.permits(ip(denied)), "{denied} should be denied");
        }
    }

    /// The mapped spellings are where this kind of check is usually bypassed.
    #[test]
    fn mapped_ipv6_spellings_are_judged_as_the_ipv4_address() {
        let policy = EgressAddressPolicy::default();
        for denied in [
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::ffff:0.0.0.0",
        ] {
            assert!(!policy.permits(ip(denied)), "{denied} should be denied");
        }
        assert!(policy.permits(ip("::ffff:93.184.216.34")));
    }

    #[test]
    fn private_ranges_are_permitted_by_default_and_deniable() {
        let default = EgressAddressPolicy::default();
        let strict = EgressAddressPolicy {
            allow_private: false,
            ..Default::default()
        };
        for private in [
            "10.0.0.5",
            "172.16.0.1",
            "192.168.1.1",
            "100.64.0.1",
            "fd00::1",
        ] {
            assert!(default.permits(ip(private)), "{private} should be allowed");
            assert!(!strict.permits(ip(private)), "{private} should be denied");
        }
    }

    #[test]
    fn ordinary_routable_addresses_pass() {
        let policy = EgressAddressPolicy::default();
        for ok in [
            "93.184.216.34",
            "8.8.8.8",
            "2606:2800:220:1:248:1893:25c8:1946",
        ] {
            assert!(policy.permits(ip(ok)), "{ok} should be allowed");
        }
    }

    #[test]
    fn the_permissive_policy_allows_what_the_default_denies() {
        let policy = EgressAddressPolicy::permissive();
        assert!(policy.permits(ip("127.0.0.1")));
        assert!(policy.permits(ip("169.254.169.254")));
    }

    #[test]
    fn the_metadata_addresses_are_denied_by_default() {
        let policy = EgressAddressPolicy::default();
        assert!(!policy.permits(IpAddr::V4(METADATA_V4)));
        assert!(!policy.permits(IpAddr::V6(METADATA_V6)));
    }

    fn sock(s: &str) -> SocketAddr {
        s.parse().expect("test socket address should parse")
    }

    fn gate(
        mode: EgressMode,
        egress_addrs: EgressAddressPolicy,
        host_owned_ports: Option<Arc<PortTable>>,
    ) -> (EgressAddressGate, Arc<PolicyMeters>) {
        let meters = Arc::new(PolicyMeters::default());
        let gate = EgressAddressGate::from_socket_policy(&SocketPolicy {
            egress_mode: mode,
            egress_addrs,
            host_owned_ports,
            meters: Some(Arc::clone(&meters)),
            ..Default::default()
        });
        (gate, meters)
    }

    #[test]
    fn the_gate_keeps_permitted_addresses_and_drops_the_rest() {
        let (gate, meters) = gate(EgressMode::Enforce, EgressAddressPolicy::default(), None);
        // A name resolving to both the metadata service and a real host is
        // dialed only at the real host, in the order the resolver gave.
        let permitted = gate
            .filter([
                sock("169.254.169.254:80"),
                sock("93.184.216.34:80"),
                sock("[::1]:80"),
                sock("[2606:2800:220:1:248:1893:25c8:1946]:80"),
            ])
            .unwrap();
        assert_eq!(
            permitted,
            [
                sock("93.184.216.34:80"),
                sock("[2606:2800:220:1:248:1893:25c8:1946]:80")
            ]
        );
        assert_eq!(meters.denied(DenyReason::BlockedRange), 2);
    }

    #[test]
    fn the_gate_refuses_when_nothing_survives() {
        let (gate, _) = gate(EgressMode::Enforce, EgressAddressPolicy::default(), None);
        for denied in [
            "127.0.0.1:80",
            "[::1]:80",
            "169.254.169.254:80",
            "[::ffff:169.254.169.254]:80",
            "[fd00:ec2::254]:80",
        ] {
            assert_eq!(
                gate.filter([sock(denied)]),
                Err(EgressRefused(DenyReason::BlockedRange)),
                "{denied} should be refused"
            );
        }
    }

    /// Count mode is the default rollout posture: nothing is severed, and
    /// every refusal enforcement would make is on the counter.
    #[test]
    fn count_mode_keeps_everything_and_counts_what_it_would_refuse() {
        let (gate, meters) = gate(EgressMode::Count, EgressAddressPolicy::default(), None);
        let candidates = [sock("127.0.0.1:80"), sock("169.254.169.254:80")];
        assert_eq!(gate.filter(candidates).unwrap(), candidates);
        assert_eq!(meters.would_deny(DenyReason::BlockedRange), 2);
        assert_eq!(meters.denied(DenyReason::BlockedRange), 0);
    }

    /// With special ranges permitted, the host's own ports still are not:
    /// whatever loopback spelling reaches them, and an exact address a plugin
    /// bound directly.
    #[test]
    fn host_owned_ports_are_refused_whatever_reaches_them() {
        let table = PortTable::new();
        let _ingress = table
            .reserve(
                Protocol::Tcp,
                sock("127.0.0.1:8000"),
                crate::host::ports::PortOwner::Host("ingress".into()),
            )
            .unwrap();
        let _direct = table
            .reserve(
                Protocol::Tcp,
                sock("10.0.0.5:50051"),
                crate::host::ports::PortOwner::Plugin("grpc".into()),
            )
            .unwrap();
        let (gate, meters) = gate(
            EgressMode::Enforce,
            EgressAddressPolicy::permissive(),
            Some(table),
        );
        for owned in [
            "127.0.0.1:8000",
            "127.0.0.2:8000",
            "[::1]:8000",
            "[::ffff:127.0.0.1]:8000",
            "0.0.0.0:8000",
            "10.0.0.5:50051",
        ] {
            assert_eq!(
                gate.filter([sock(owned)]),
                Err(EgressRefused(DenyReason::HostOwnedPort)),
                "{owned} should be refused"
            );
        }
        assert_eq!(meters.denied(DenyReason::HostOwnedPort), 6);
        for free in ["127.0.0.1:9000", "10.0.0.5:8000", "10.0.0.6:50051"] {
            assert!(gate.filter([sock(free)]).is_ok(), "{free} should pass");
        }
    }

    /// Ingress bound to every interface is reached through the machine's own
    /// interface address, such as a pod IP, which the default range policy
    /// allows as private. A name resolving there is refused like loopback.
    #[test]
    fn a_wildcard_listener_is_refused_at_the_machines_own_address() {
        let table = PortTable::new();
        let _ingress = table
            .reserve(
                Protocol::Tcp,
                sock("0.0.0.0:8000"),
                crate::host::ports::PortOwner::Host("ingress".into()),
            )
            .unwrap();
        let (gate, _) = gate(
            EgressMode::Enforce,
            EgressAddressPolicy::default(),
            Some(table),
        );
        // The source address the kernel picks for a route out; nothing is sent.
        let own = std::net::UdpSocket::bind("0.0.0.0:0")
            .and_then(|s| s.connect("192.0.2.1:9").and_then(|()| s.local_addr()))
            .map(|a| a.ip())
            .ok()
            .filter(|ip| !ip.is_loopback() && !ip.is_unspecified());
        let Some(own) = own else {
            eprintln!("no non-loopback interface address; nothing to check");
            return;
        };
        assert_eq!(
            gate.filter([SocketAddr::new(own, 8000)]),
            Err(EgressRefused(DenyReason::HostOwnedPort)),
            "{own}:8000 is this machine's ingress"
        );
        assert!(gate.filter([SocketAddr::new(own, 8001)]).is_ok());
    }
}
