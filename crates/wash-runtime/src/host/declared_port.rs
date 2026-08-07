//! A port a guest listens on, declared in configuration rather than by the
//! guest itself.
//!
//! A guest never chooses its own exposure. It binds a port inside its virtual
//! loopback and the host splices a real listener into it
//! ([`PortMode::Splice`]), or — for a host component plugin only, and when the
//! extra copy that splice costs is worth avoiding — the operator names a
//! concrete address the plugin may bind for real ([`PortMode::Direct`]).
//! Either way the address and port come from configuration, not from guest
//! code.
//!
//! Two declaration surfaces share this type:
//!
//! - **`host.hostPlugins[].ports`** in the `wash host` config file, for a host
//!   component plugin. There is no `--host-plugin` flag spelling: that syntax
//!   is a comma-separated `key=value` list and cannot carry a nested list, so
//!   its `FromStr` rejects a `port=` field and points at the config file.
//! - **`service.ports`** on a workload, arriving over the wire from a
//!   `WorkloadDeployment` or `wash` YAML. A workload's ports come from a
//!   tenant rather than the operator, so [`validate_workload_ports`] refuses
//!   `bind`: only an operator gets to hand a guest a real listening socket.
//!
//! `publish` and `bind` pick the mode between them, following the same
//! exactly-one-of shape `image`/`file` already use for a component source:
//! `bind` present means the plugin binds that address itself, `publish` present
//! means the host exposes it, and neither means the port is declared but not
//! reachable.

use core::net::IpAddr;

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

/// Transport for a declared port.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Protocol {
    #[default]
    #[serde(alias = "tcp")]
    Tcp,
    #[serde(alias = "udp")]
    Udp,
}

impl Protocol {
    pub fn is_udp(self) -> bool {
        matches!(self, Self::Udp)
    }
}

impl core::fmt::Display for Protocol {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Tcp => f.write_str("TCP"),
            Self::Udp => f.write_str("UDP"),
        }
    }
}

/// How a declared port reaches the outside world, resolved from
/// [`DeclaredPort`]'s `publish`/`bind` fields by [`DeclaredPort::mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortMode {
    /// The plugin binds its virtual loopback; nothing outside can reach it.
    /// The default, and exactly the behavior a plugin has today.
    Declared,
    /// The plugin binds its virtual loopback and the host binds the real port
    /// carried here, splicing accepted connections into it.
    Splice { publish: u16 },
    /// The plugin binds this concrete address itself, holding the real
    /// listening socket. The operator names the address, so this still cannot
    /// become "every interface".
    Direct { bind: IpAddr },
}

/// One entry of a plugin's `ports` list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeclaredPort {
    /// Names this port within the plugin. Appears in logs, metrics, and the
    /// conflict error when two declarations collide.
    pub name: String,
    /// The port the plugin's own code binds — on its virtual loopback under
    /// [`PortMode::Splice`], on the declared address under
    /// [`PortMode::Direct`]. There is only ever one number here, so a plugin
    /// author and an operator cannot disagree about it.
    pub port: u16,
    #[serde(default)]
    pub protocol: Protocol,
    /// Real port the host exposes, splicing into the plugin's virtual
    /// loopback. Mutually exclusive with `bind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<u16>,
    /// Concrete address the plugin binds directly, skipping the splice.
    /// Mutually exclusive with `publish`. An unspecified or loopback address is
    /// rejected — see [`DeclaredPort::validate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<IpAddr>,
}

impl DeclaredPort {
    /// How this declaration is exposed. Only meaningful once
    /// [`DeclaredPort::validate`] has accepted it; a declaration setting both
    /// `publish` and `bind` reports [`PortMode::Direct`] here and is rejected
    /// there.
    pub fn mode(&self) -> PortMode {
        match (self.bind, self.publish) {
            (Some(bind), _) => PortMode::Direct { bind },
            (None, Some(publish)) => PortMode::Splice { publish },
            (None, None) => PortMode::Declared,
        }
    }

    /// The real port this declaration exposes, if any.
    pub fn published_port(&self) -> Option<u16> {
        match self.mode() {
            PortMode::Declared => None,
            PortMode::Splice { publish } => Some(publish),
            PortMode::Direct { .. } => Some(self.port),
        }
    }

    /// Check a declaration in isolation. `owner` leads every error and should
    /// name something the operator can find in their config, e.g.
    /// `host_plugins 'grpc-gateway'`.
    ///
    /// # Errors
    ///
    /// Rejects an empty name, a zero port, both `publish` and `bind`, and —
    /// the one that matters for exposure — a `bind` that is unspecified or
    /// loopback.
    pub fn validate(&self, owner: &str) -> Result<()> {
        ensure!(
            !self.name.trim().is_empty(),
            "{owner}: a ports entry needs a non-empty `name`"
        );
        let name = &self.name;
        ensure!(
            self.port != 0,
            "{owner}: port '{name}' must declare a non-zero `port`"
        );
        ensure!(
            !(self.publish.is_some() && self.bind.is_some()),
            "{owner}: port '{name}' sets both `publish` and `bind`. `publish` has the host expose \
             the port and splice into the plugin; `bind` has the plugin hold the real socket \
             itself. Pick one"
        );

        match self.mode() {
            PortMode::Declared => {}
            PortMode::Splice { publish } => {
                ensure!(
                    publish != 0,
                    "{owner}: port '{name}' has `publish: 0`; omit `publish` to declare a port \
                     without exposing it"
                );
            }
            PortMode::Direct { bind } => {
                if bind.is_unspecified() {
                    bail!(
                        "{owner}: port '{name}' has `bind: {bind}`, which would listen on every \
                         interface. Name the address to expose, so the host rather than the \
                         plugin decides what is reachable"
                    );
                }
                if bind.is_loopback() {
                    bail!(
                        "{owner}: port '{name}' has `bind: {bind}`. A loopback bind is always \
                         routed to the plugin's private virtual network, so it would not produce \
                         a real socket. Drop `bind` and set `publish` to expose this port, or \
                         name a non-loopback address"
                    );
                }
                if bind.is_multicast() {
                    bail!(
                        "{owner}: port '{name}' has `bind: {bind}`, which is a multicast address"
                    );
                }
            }
        }
        Ok(())
    }
}

/// Check a workload's `ports` list.
///
/// Same as [`validate_ports`], plus: a workload may not use `bind`. Direct
/// binds hand the guest a real listening socket on an address it names, which
/// is only ever an operator's call — a workload's ports come from a tenant.
///
/// # Errors
///
/// Returns the first problem found, named so the author can find it.
pub fn validate_workload_ports(ports: &[DeclaredPort], owner: &str) -> Result<()> {
    for port in ports {
        if port.bind.is_some() {
            bail!(
                "{owner}: port '{}' sets `bind`, which is only available to host component                  plugins. A workload's ports are published by the host; set `publish` instead",
                port.name
            );
        }
    }
    validate_ports(ports, owner)
}

/// Check a whole `ports` list: each entry on its own, then the list for
/// duplicate names and for two entries claiming the same real port.
///
/// Collisions *between* plugins are not visible here — those surface when each
/// port is reserved from the host's one port table.
///
/// # Errors
///
/// Returns the first problem found, named so an operator can find it in their
/// config file.
pub fn validate_ports(ports: &[DeclaredPort], owner: &str) -> Result<()> {
    let mut seen_names = std::collections::BTreeSet::new();
    let mut seen_published = std::collections::BTreeMap::new();

    for port in ports {
        port.validate(owner)?;
        ensure!(
            seen_names.insert(port.name.as_str()),
            "{owner}: two ports are both named '{}'",
            port.name
        );
        if let Some(published) = port.published_port()
            && let Some(other) = seen_published.insert((published, port.protocol), &port.name)
        {
            bail!(
                "{owner}: ports '{other}' and '{}' both claim {} port {published}",
                port.name,
                port.protocol
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> DeclaredPort {
        serde_json::from_str(json).expect("test declaration should parse")
    }

    fn splice(name: &str, port: u16, publish: Option<u16>) -> DeclaredPort {
        DeclaredPort {
            name: name.into(),
            port,
            protocol: Protocol::Tcp,
            publish,
            bind: None,
        }
    }

    fn direct(name: &str, port: u16, bind: &str) -> DeclaredPort {
        DeclaredPort {
            name: name.into(),
            port,
            protocol: Protocol::Tcp,
            publish: None,
            bind: Some(bind.parse().expect("test address should parse")),
        }
    }

    #[test]
    fn a_declared_but_unpublished_port_is_the_default() {
        let parsed = parse(r#"{"name":"grpc","port":50051}"#);
        assert_eq!(parsed, splice("grpc", 50051, None));
        assert_eq!(parsed.mode(), PortMode::Declared);
        assert_eq!(parsed.published_port(), None);
        parsed.validate("test").unwrap();
    }

    #[test]
    fn publish_and_bind_select_the_mode_between_them() {
        let published = parse(r#"{"name":"grpc","port":50051,"protocol":"TCP","publish":31051}"#);
        assert_eq!(published, splice("grpc", 50051, Some(31051)));
        assert_eq!(published.mode(), PortMode::Splice { publish: 31051 });
        assert_eq!(published.published_port(), Some(31051));
        published.validate("test").unwrap();

        let direct_port = parse(r#"{"name":"bulk","port":9000,"bind":"10.0.0.5"}"#);
        assert_eq!(direct_port, direct("bulk", 9000, "10.0.0.5"));
        // A direct bind is exposed on the port the plugin binds, not a
        // separately chosen one.
        assert_eq!(direct_port.published_port(), Some(9000));
        direct_port.validate("test").unwrap();
    }

    #[test]
    fn protocol_accepts_either_case_and_defaults_to_tcp() {
        assert_eq!(
            parse(r#"{"name":"a","port":1,"protocol":"udp"}"#).protocol,
            Protocol::Udp
        );
        assert_eq!(
            parse(r#"{"name":"a","port":1,"protocol":"UDP"}"#).protocol,
            Protocol::Udp
        );
        assert_eq!(parse(r#"{"name":"a","port":1}"#).protocol, Protocol::Tcp);
    }

    #[test]
    fn a_misspelled_field_is_rejected_rather_than_ignored() {
        // `deny_unknown_fields`: a typo in an exposure field must not silently
        // leave the port unexposed.
        assert!(
            serde_json::from_str::<DeclaredPort>(r#"{"name":"a","port":1,"publsh":2}"#).is_err()
        );
    }

    #[test]
    fn setting_both_publish_and_bind_is_rejected() {
        let mut port = splice("bulk", 9000, Some(31000));
        port.bind = Some("10.0.0.5".parse().unwrap());
        let err = port.validate("p").unwrap_err().to_string();
        assert!(err.contains("Pick one"), "got: {err}");
    }

    #[test]
    fn a_direct_bind_to_every_interface_is_rejected() {
        for bind in ["0.0.0.0", "::"] {
            let err = direct("bulk", 9000, bind)
                .validate("p")
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("every interface"),
                "unexpected error for {bind}: {err}"
            );
        }
    }

    /// A loopback address never reaches the OS on the bind path — it is routed
    /// to the plugin's virtual network — so accepting it here would hand the
    /// operator a listener that silently is not real.
    #[test]
    fn a_direct_bind_to_loopback_is_rejected_with_the_reason() {
        let err = direct("bulk", 9000, "127.0.0.1")
            .validate("p")
            .unwrap_err()
            .to_string();
        assert!(err.contains("private virtual network"), "got: {err}");
    }

    #[test]
    fn a_zero_port_is_rejected() {
        assert!(splice("grpc", 0, None).validate("p").is_err());
        assert!(splice("grpc", 50051, Some(0)).validate("p").is_err());
    }

    /// UDP publishes through the datagram relay, so all three shapes are
    /// accepted: declared-only, published, and a direct bind.
    #[test]
    fn udp_is_accepted_in_every_shape() {
        let mut port = splice("dns", 5353, Some(31053));
        port.protocol = Protocol::Udp;
        port.validate("p").unwrap();
        assert_eq!(port.published_port(), Some(31053));

        port.publish = None;
        port.validate("p").unwrap();

        port.bind = Some("10.0.0.5".parse().unwrap());
        port.validate("p").unwrap();
    }

    #[test]
    fn duplicate_names_and_duplicate_published_ports_are_rejected() {
        let dup_name = [splice("a", 1, None), splice("a", 2, None)];
        assert!(
            validate_ports(&dup_name, "p")
                .unwrap_err()
                .to_string()
                .contains("both named")
        );

        let dup_port = [splice("a", 1, Some(31000)), splice("b", 2, Some(31000))];
        assert!(
            validate_ports(&dup_port, "p")
                .unwrap_err()
                .to_string()
                .contains("both claim TCP port 31000")
        );

        // A direct bind collides with a splice publishing the same number.
        let mixed = [splice("a", 9000, Some(9000)), direct("b", 9000, "10.0.0.5")];
        assert!(validate_ports(&mixed, "p").is_err());
    }

    #[test]
    fn the_same_number_on_different_protocols_is_not_a_collision() {
        let mut udp = splice("dns", 53, None);
        udp.protocol = Protocol::Udp;
        validate_ports(&[splice("http", 53, Some(31053)), udp], "p").unwrap();
    }
}
