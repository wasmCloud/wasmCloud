//! The reserved `*.wasmcloud.internal` name zone.
//!
//! `127.0.0.1` means two different things depending on which API a guest asks
//! through, and neither meaning can be spelled. This zone gives both a name:
//!
//! | Name                          | Reaches                                  |
//! | ----------------------------- | ---------------------------------------- |
//! | `service.wasmcloud.internal`  | the workload's own service, virtually     |
//! | `<n>.svc.wasmcloud.internal`  | reserved spelling for a named service     |
//! | `host.wasmcloud.internal`     | the real loopback of the machine          |
//!
//! `localhost` and `127.0.0.1` keep meaning the virtual network. Nothing here
//! changes what already works.
//!
//! # Resolved inside the host, never on the wire
//!
//! Resolution is intercepted before `allowedIpNameLookups` and before any
//! resolver is consulted. That allowlist exists because resolution reaches the
//! network before a connection is attempted, so a guest can encode data in the
//! labels it looks up; these names never leave the process, so they carry that
//! risk and need that gate. A workload should not have to open DNS to talk to
//! its own service.
//!
//! Reaching the *host* is still gated — at connect, where it belongs, by
//! `allowedHostLoopback` plus the host's own `--allow-host-loopback`.
//!
//! # Why the sentinel lives inside `127.0.0.0/8`
//!
//! `connect` takes an address, not a name, so resolution has to hand back
//! something the connect path recognizes. Which address is a safety decision:
//! a link-local sentinel (`169.254.x.x`) would leave the machine as a real
//! packet if any interception were ever missed, potentially onto a shared
//! segment. [`HOST_SENTINEL`] cannot: every path treats `127.x` as the virtual
//! network, which refuses anything not registered in it, so a missed
//! interception degrades to `connection-refused` — the right answer for a guest
//! without the policy.
//!
//! `127.255.255.254` specifically because nothing binds it in practice, while
//! `127.0.0.2` and its neighbours are common for local test servers.

use core::net::{IpAddr, Ipv4Addr, SocketAddr};

/// The address `host.wasmcloud.internal` resolves to. Rewritten to real
/// loopback at connect; see the module docs for why it lives in `127.0.0.0/8`.
pub const HOST_SENTINEL: Ipv4Addr = Ipv4Addr::new(127, 255, 255, 254);

/// The zone suffix, without a leading dot.
pub const ZONE: &str = "wasmcloud.internal";

/// What a name in the zone resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InternalName {
    /// The workload's own service, on its virtual loopback.
    Service,
    /// The machine the host process runs on.
    Host,
}

impl InternalName {
    /// The address this name answers with.
    pub fn address(self) -> IpAddr {
        match self {
            Self::Service => IpAddr::V4(Ipv4Addr::LOCALHOST),
            Self::Host => IpAddr::V4(HOST_SENTINEL),
        }
    }
}

/// Resolve `name` within the reserved zone.
///
/// Returns `None` for anything outside it, which the caller passes on to the
/// ordinary `allowedIpNameLookups` + resolver path. Returns
/// `Some(Err(UnknownInternalName))` for a name that *is* in the zone but names
/// nothing — those must fail rather than fall through, or a cluster running a
/// real `wasmcloud.internal` zone could answer for a label we do not define.
///
/// Comparison is ASCII-case-insensitive, and a single trailing root dot is
/// ignored so the fully-qualified spelling resolves the same.
pub fn resolve(name: &str) -> Option<Result<InternalName, UnknownInternalName>> {
    let name = name.strip_suffix('.').unwrap_or(name);
    // Lowercased once so every comparison below is case-insensitive, including
    // the zone suffix itself.
    let name = name.to_ascii_lowercase();
    let label = name.strip_suffix(ZONE)?;
    // `strip_suffix` also matches something like `notwasmcloud.internal`; the
    // remainder must be empty or end at a label boundary.
    let label = match label {
        "" => return Some(Err(UnknownInternalName)),
        l => l.strip_suffix('.')?,
    };

    if label == "service" {
        return Some(Ok(InternalName::Service));
    }
    if label == "host" {
        return Some(Ok(InternalName::Host));
    }
    // `<name>.svc` is reserved for named services. A workload has exactly one
    // service today, so every spelling answers the same address; accepting it
    // now means manifests written against it keep working when several
    // services become possible.
    if let Some(service) = label.strip_suffix(".svc")
        && !service.is_empty()
        && !service.contains('.')
    {
        return Some(Ok(InternalName::Service));
    }
    Some(Err(UnknownInternalName))
}

/// A name inside the reserved zone that names nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnknownInternalName;

/// Whether `addr` is the host sentinel, in either its v4 or v4-mapped-v6 form.
///
/// A guest on a v6 socket that resolved an A record sees the mapped spelling, so
/// checking only the v4 form would silently send it to the virtual network.
#[must_use]
pub fn is_host_sentinel(addr: IpAddr) -> bool {
    match addr.to_canonical() {
        IpAddr::V4(v4) => v4 == HOST_SENTINEL,
        IpAddr::V6(_) => false,
    }
}

/// Rewrite a sentinel address to the real loopback address to dial, preserving
/// the port and matching the family the socket is using.
#[must_use]
pub fn rewrite_sentinel(addr: SocketAddr) -> SocketAddr {
    let ip = match addr {
        SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
        // A v6 socket cannot connect to a v4 address, so the v4-mapped sentinel
        // becomes the v6 loopback rather than `127.0.0.1`.
        SocketAddr::V6(_) => IpAddr::V6(core::net::Ipv6Addr::LOCALHOST),
    };
    SocketAddr::new(ip, addr.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zone_resolves_its_two_destinations() {
        assert_eq!(
            resolve("service.wasmcloud.internal"),
            Some(Ok(InternalName::Service))
        );
        assert_eq!(
            resolve("host.wasmcloud.internal"),
            Some(Ok(InternalName::Host))
        );
        assert_eq!(
            InternalName::Service.address(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_eq!(InternalName::Host.address(), IpAddr::V4(HOST_SENTINEL));
    }

    #[test]
    fn resolution_is_case_insensitive_and_ignores_the_root_dot() {
        for name in [
            "SERVICE.WASMCLOUD.INTERNAL",
            "Service.WasmCloud.Internal",
            "service.wasmcloud.internal.",
        ] {
            assert_eq!(resolve(name), Some(Ok(InternalName::Service)), "{name}");
        }
    }

    #[test]
    fn named_services_are_reserved_syntax_answering_the_same_address() {
        assert_eq!(
            resolve("api.svc.wasmcloud.internal"),
            Some(Ok(InternalName::Service))
        );
        // Not a single label before `.svc`.
        assert_eq!(
            resolve("a.b.svc.wasmcloud.internal"),
            Some(Err(UnknownInternalName))
        );
        assert_eq!(
            resolve(".svc.wasmcloud.internal"),
            Some(Err(UnknownInternalName))
        );
    }

    /// A name inside the zone that we do not define must fail here, not fall
    /// through to DNS — otherwise a cluster running a real `wasmcloud.internal`
    /// zone could answer for it.
    #[test]
    fn an_unknown_name_in_the_zone_does_not_fall_through() {
        assert_eq!(
            resolve("nope.wasmcloud.internal"),
            Some(Err(UnknownInternalName))
        );
        assert_eq!(
            resolve("wasmcloud.internal"),
            Some(Err(UnknownInternalName))
        );
    }

    #[test]
    fn names_outside_the_zone_are_left_alone() {
        for name in [
            "example.com",
            "wasmcloud.internal.example.com",
            "notwasmcloud.internal",
            "localhost",
        ] {
            assert_eq!(resolve(name), None, "{name} should not be intercepted");
        }
    }

    /// The v4-mapped spelling is where this kind of check is usually bypassed.
    #[test]
    fn the_sentinel_is_recognised_in_both_spellings() {
        assert!(is_host_sentinel(IpAddr::V4(HOST_SENTINEL)));
        assert!(is_host_sentinel(
            "::ffff:127.255.255.254".parse::<IpAddr>().unwrap()
        ));
        assert!(!is_host_sentinel(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(!is_host_sentinel("::1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn rewriting_keeps_the_port_and_matches_the_socket_family() {
        let v4: SocketAddr = "127.255.255.254:5432".parse().unwrap();
        assert_eq!(
            rewrite_sentinel(v4),
            "127.0.0.1:5432".parse::<SocketAddr>().unwrap()
        );

        let v6: SocketAddr = "[::ffff:127.255.255.254]:5432".parse().unwrap();
        assert_eq!(
            rewrite_sentinel(v6),
            "[::1]:5432".parse::<SocketAddr>().unwrap()
        );
    }
}
