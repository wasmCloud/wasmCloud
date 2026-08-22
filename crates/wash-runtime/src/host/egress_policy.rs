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

use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

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
}
