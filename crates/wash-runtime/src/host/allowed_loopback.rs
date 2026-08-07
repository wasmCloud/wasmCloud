//! Port allowlist for reaching the machine's own loopback.
//!
//! A guest's `127.0.0.1` means its own virtual network, not the machine the
//! host runs on. `host.wasmcloud.internal` names the machine — the Docker
//! `host.docker.internal` idea — and this allowlist is what decides whether a
//! given guest may go through that door, and to which ports.
//!
//! # Accepted forms
//!
//! | Form        | Meaning                     |
//! | ----------- | --------------------------- |
//! | `5432`      | TCP port 5432 (the default) |
//! | `5432/tcp`  | TCP port 5432               |
//! | `53/udp`    | UDP port 53                 |
//!
//! Deliberately narrow: no ranges, no `*`, no names. Reaching the host's own
//! loopback is the most privileged thing this policy grants — a range would
//! make it easy to hand over a whole class of local service by accident, and
//! the set of local ports a guest legitimately needs is small and knowable.
//!
//! # Empty list denies every connection
//!
//! Same shape as its neighbours: an empty or absent list denies every
//! host-loopback connection. The workload's declaration is only half of it —
//! the host must also be started with `--allow-host-loopback`, so neither a
//! workload author nor an operator can open this door alone.

use core::net::SocketAddr;
use std::fmt;
use std::str::FromStr;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize, de, ser};

use crate::host::declared_port::Protocol;

/// One entry of the `allowedHostLoopback` allowlist: a port and its transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AllowedLoopbackPort {
    pub port: u16,
    pub protocol: Protocol,
}

impl AllowedLoopbackPort {
    pub fn tcp(port: u16) -> Self {
        Self {
            port,
            protocol: Protocol::Tcp,
        }
    }

    pub fn udp(port: u16) -> Self {
        Self {
            port,
            protocol: Protocol::Udp,
        }
    }

    /// Whether this entry permits reaching `addr` over `protocol`.
    ///
    /// Only the port is compared: the address has already been established to
    /// be the machine's loopback by the time this runs, and every loopback
    /// address reaches the same listener set.
    #[must_use]
    pub fn permits(&self, addr: SocketAddr, protocol: Protocol) -> bool {
        self.port == addr.port() && self.protocol == protocol
    }
}

/// Returns `true` if `addr` over `protocol` satisfies any entry in `policy`.
///
/// An empty `policy` denies every host-loopback connection.
#[must_use]
pub fn check_allowed_loopback(
    policy: &[AllowedLoopbackPort],
    addr: SocketAddr,
    protocol: Protocol,
) -> bool {
    policy.iter().any(|entry| entry.permits(addr, protocol))
}

impl FromStr for AllowedLoopbackPort {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            bail!("host-loopback entry is empty");
        }
        if trimmed.contains('-') || trimmed.contains("..") {
            bail!(
                "host-loopback entry {trimmed:?} looks like a range; list each port separately, so \
                 granting access to the host's own loopback stays deliberate"
            );
        }
        if trimmed.contains('*') {
            bail!(
                "host-loopback entry {trimmed:?} uses a wildcard; reaching the host's loopback \
                 must name each port"
            );
        }

        let (port, protocol) = match trimmed.split_once('/') {
            Some((port, proto)) => {
                let protocol = match proto.trim().to_ascii_lowercase().as_str() {
                    "tcp" => Protocol::Tcp,
                    "udp" => Protocol::Udp,
                    other => bail!(
                        "host-loopback entry {trimmed:?} has unknown protocol {other:?}; \
                         expected tcp or udp"
                    ),
                };
                (port.trim(), protocol)
            }
            None => (trimmed, Protocol::Tcp),
        };

        let port: u16 = port
            .parse()
            .map_err(|e| anyhow!("host-loopback entry {trimmed:?} has an invalid port: {e}"))?;
        if port == 0 {
            bail!("host-loopback entry {trimmed:?} must name a non-zero port");
        }
        Ok(Self { port, protocol })
    }
}

impl fmt::Display for AllowedLoopbackPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.protocol {
            // TCP is the default, so it round-trips as the bare port.
            Protocol::Tcp => write!(f, "{}", self.port),
            Protocol::Udp => write!(f, "{}/udp", self.port),
        }
    }
}

impl Serialize for AllowedLoopbackPort {
    fn serialize<S: ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for AllowedLoopbackPort {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> AllowedLoopbackPort {
        s.parse().expect("entry should parse")
    }

    #[test]
    fn a_bare_port_is_tcp() {
        assert_eq!(parse("5432"), AllowedLoopbackPort::tcp(5432));
        assert_eq!(parse("5432/tcp"), AllowedLoopbackPort::tcp(5432));
        assert_eq!(parse(" 5432 / TCP "), AllowedLoopbackPort::tcp(5432));
        assert_eq!(parse("53/udp"), AllowedLoopbackPort::udp(53));
    }

    #[test]
    fn round_trips_through_its_string_form() {
        for entry in ["5432", "53/udp"] {
            assert_eq!(parse(entry).to_string(), entry);
        }
        // TCP is the default, so it renders bare rather than as `/tcp`.
        assert_eq!(parse("5432/tcp").to_string(), "5432");
    }

    #[test]
    fn ranges_wildcards_and_names_are_rejected() {
        for entry in ["5000-6000", "*", "5432-", "postgres", "", "0", "70000"] {
            assert!(
                entry.parse::<AllowedLoopbackPort>().is_err(),
                "{entry:?} should not parse"
            );
        }
    }

    #[test]
    fn an_unknown_protocol_is_rejected() {
        let err = "53/sctp"
            .parse::<AllowedLoopbackPort>()
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected tcp or udp"), "got: {err}");
    }

    #[test]
    fn matching_compares_port_and_protocol_only() {
        let policy = [AllowedLoopbackPort::tcp(5432)];
        let pg: SocketAddr = "127.0.0.1:5432".parse().unwrap();
        assert!(check_allowed_loopback(&policy, pg, Protocol::Tcp));
        // Same port, wrong transport.
        assert!(!check_allowed_loopback(&policy, pg, Protocol::Udp));
        // Wrong port.
        assert!(!check_allowed_loopback(
            &policy,
            "127.0.0.1:6379".parse().unwrap(),
            Protocol::Tcp
        ));
        // Any loopback spelling reaches the same listeners, so the address bits
        // do not participate.
        assert!(check_allowed_loopback(
            &policy,
            "127.0.0.2:5432".parse().unwrap(),
            Protocol::Tcp
        ));
    }

    #[test]
    fn an_empty_policy_denies_everything() {
        assert!(!check_allowed_loopback(
            &[],
            "127.0.0.1:5432".parse().unwrap(),
            Protocol::Tcp
        ));
    }
}
