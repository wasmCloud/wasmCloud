//! Operator-owned configuration for `wasmcloud:nats` bindings.
//!
//! A workload names a binding — `(implements orders)`, or the unnamed binding
//! for a plain import — and this module decides what that binding *is*: which
//! servers it dials, as whom, and what it is granted. The workload asks for a
//! binding; it does not describe one.
//!
//! Both halves of a binding's configuration arrive as the same
//! `HashMap<String, String>` the manifest writes, and both are parsed by
//! [`super::config::NatsConfig::from_map`], so there is one parser and one set
//! of validation rules no matter which surface a key came from.
//!
//! Keys fall into three classes:
//!
//! - **Connection** ([`CONNECTION_KEYS`]) — where the binding points and as
//!   whom: servers, credentials, TLS, JetStream domain, inbox prefix.
//! - **Grant** ([`GRANT_KEYS`]) — what it may reach: `subject-allow`,
//!   `stream-allow`, `bucket-allow`.
//! - **Everything else** — what the workload does within that grant:
//!   subscriptions, ack mode, in-flight limits, request timeout.
//!
//! Under [`WorkloadConfig::Deny`] the first two classes are the host's alone. A
//! workload that sets one is refused at bind rather than silently overridden,
//! because there is no narrower reading of "point somewhere else": a manifest
//! naming its own servers means a different cluster, and a manifest naming its
//! own grants means a privilege its operator did not hand out.

use std::collections::{BTreeMap, HashMap};

use super::config::{NatsConfig, canonical_key};

/// Config keys that decide where a binding connects and as whom.
///
/// Aliases are listed alongside their canonical spelling
/// (`creds`/`creds-file`, `nkey-seed`/`nkey`, `username`/`user`) because
/// [`super::config`] reads either, so denying only one spelling would deny
/// nothing.
pub const CONNECTION_KEYS: &[&str] = &[
    "servers",
    "creds",
    "creds-file",
    "jwt",
    "nkey-seed",
    "nkey",
    "username",
    "user",
    "password",
    "token",
    "tls-ca",
    "tls-cert",
    "tls-key",
    "tls-first",
    "jetstream-domain",
    "inbox-prefix",
    "name",
];

/// Config keys that decide what a binding may reach.
pub const GRANT_KEYS: &[&str] = &["subject-allow", "stream-allow", "bucket-allow"];

/// The binding name of a plain, unlabeled import.
pub use super::conn::UNNAMED_BINDING;

/// True when `key` (canonical spelling) belongs to the host rather than the
/// workload.
fn host_owned(key: &str) -> bool {
    CONNECTION_KEYS.contains(&key) || GRANT_KEYS.contains(&key)
}

/// Who supplies a binding's connection settings and grants.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkloadConfig {
    /// A workload's own interface `config` supplies them, with the host's
    /// configuration as a default beneath it.
    ///
    /// The default, and what an embedder with no operator configuration gets:
    /// denying keys nothing supplies would leave every binding unusable. `wash
    /// dev` runs this way so a project's manifest is self-contained.
    #[default]
    Allow,
    /// Connection settings and grants come only from the host. A workload that
    /// sets one is refused at bind, and a workload that names a binding the
    /// host does not serve is refused rather than handed an empty grant.
    ///
    /// What `wash host` runs by default: the bindings an operator declared are
    /// the whole allowlist, and a manifest cannot widen its own reach.
    Deny,
}

impl WorkloadConfig {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }
}

/// The bindings a host serves, and who may configure them.
///
/// `base` applies to every binding; a named entry layers on top of it. Storing
/// both as the plain string map a manifest writes is deliberate — the host's
/// configuration and a workload's are merged and then parsed by exactly the
/// same code, so an operator cannot write a value the manifest parser would
/// have rejected.
#[derive(Debug, Clone, Default)]
pub struct NatsBindings {
    base: HashMap<String, String>,
    bindings: BTreeMap<String, HashMap<String, String>>,
    workload_config: WorkloadConfig,
}

/// Canonicalizes every key of a config map, so a host config written in
/// snake_case denies the kebab-case spelling too.
fn canonicalize(config: HashMap<String, String>) -> HashMap<String, String> {
    config
        .into_iter()
        .map(|(key, value)| (canonical_key(&key), value))
        .collect()
}

impl NatsBindings {
    /// Host configuration that describes nothing and denies nothing: every
    /// binding is exactly what its workload's manifest says.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets who supplies connection settings and grants.
    pub fn with_workload_config(mut self, workload_config: WorkloadConfig) -> Self {
        self.workload_config = workload_config;
        self
    }

    /// Configuration applied to every binding, named or not.
    pub fn with_base(mut self, config: HashMap<String, String>) -> Self {
        self.base = canonicalize(config);
        self
    }

    /// The servers a binding dials when nothing else names any.
    ///
    /// The host's own data plane (`--wasmcloud-nats-url`, falling back to
    /// `--data-nats-url`), so a workload on the cluster's NATS needs no address
    /// in its manifest and the same manifest runs in dev and on a cluster.
    ///
    /// Deliberately address-only: it carries no credentials and no grant, so a
    /// workload that inherits the address still reaches nothing until it is
    /// granted something.
    pub fn with_default_servers(mut self, servers: Vec<String>) -> Self {
        if !servers.is_empty() {
            // A fallback, not an override: an operator who wrote `servers` into
            // the host's own configuration meant that address, and the flag it
            // would otherwise clobber has a value on every `wash host`.
            self.base
                .entry("servers".to_string())
                .or_insert_with(|| servers.join(","));
        }
        self
    }

    /// Configuration for one `(implements ..)` name, layered over [`Self::with_base`].
    pub fn with_binding(
        mut self,
        name: impl Into<String>,
        config: HashMap<String, String>,
    ) -> Self {
        self.bindings.insert(name.into(), canonicalize(config));
        self
    }

    /// Whether a workload may describe its own connection and grants.
    pub fn workload_config(&self) -> WorkloadConfig {
        self.workload_config
    }

    /// The names the host describes, for a startup log.
    pub fn binding_names(&self) -> Vec<&str> {
        self.bindings.keys().map(String::as_str).collect()
    }

    /// The host's own configuration for `binding`: `base`, with the named
    /// entry layered over it.
    fn host_layer(&self, binding: &str) -> HashMap<String, String> {
        let mut layer = self.base.clone();
        if let Some(named) = self.bindings.get(binding) {
            layer.extend(named.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        layer
    }

    /// The configuration a binding opens with.
    ///
    /// # Errors
    ///
    /// Under [`WorkloadConfig::Deny`], fails when the workload sets a key the
    /// host owns, or names a binding the host does not serve.
    pub fn resolve(
        &self,
        binding: &str,
        workload: HashMap<String, String>,
    ) -> anyhow::Result<HashMap<String, String>> {
        // Canonicalized first: `subject_allow` and `subject-allow` are one key
        // to the parser, so a check that read only the kebab-case spelling
        // would deny nothing.
        let workload = canonicalize(workload);
        let mut resolved = self.host_layer(binding);

        if self.workload_config == WorkloadConfig::Deny {
            // A name is a request for a binding the operator declared. Handing
            // an unknown name the base configuration instead would start the
            // workload against the right cluster with an empty grant, and every
            // call it makes would be denied one at a time with nothing pointing
            // at the missing declaration.
            if binding != UNNAMED_BINDING && !self.bindings.contains_key(binding) {
                anyhow::bail!(
                    "this host serves no wasmcloud:nats binding named `{binding}`{}. A workload \
                     asks for a binding by name and the host declares what it is; add \
                     `{binding}` under `host.wasmcloudNats.bindings` in the host's config file",
                    self.describe_available()
                )
            }

            let mut refused: Vec<String> = workload
                .keys()
                .filter(|key| host_owned(key))
                .map(|key| format!("`{key}`"))
                .collect();
            if !refused.is_empty() {
                // Sorted: the same manifest has to be refused with the same
                // message every time, and `workload` is a `HashMap`.
                refused.sort();
                anyhow::bail!(
                    "wasmcloud:nats binding `{}` sets {}, which this host does not accept from a \
                     workload. Where a binding connects, as whom, and what it may reach are the \
                     host's to declare — a manifest that set them could point itself at another \
                     cluster or widen its own grant. Ask the operator to declare them under \
                     `host.wasmcloudNats`, and keep `subscriptions`, `core-subscriptions`, \
                     `kv-watches`, and `ack-mode` in the manifest",
                    describe(binding),
                    refused.join(", ")
                )
            }
        }

        resolved.extend(workload);
        Ok(resolved)
    }

    /// The binding names an error can suggest, when there are any.
    fn describe_available(&self) -> String {
        if self.bindings.is_empty() {
            String::new()
        } else {
            format!(" (it serves {})", self.binding_names().join(", "))
        }
    }

    /// Parses every declared binding, so a bad value fails at host startup
    /// rather than at the first workload that asks for it.
    ///
    /// # Errors
    ///
    /// Fails if any declared binding is not a valid configuration.
    pub fn validate(&self) -> anyhow::Result<()> {
        // `inbox-prefix` on the base layer is every binding's inbox prefix, on
        // every workload the host runs — which is the one case
        // `conn::binding_inbox_prefix` exists to prevent: two workloads sharing
        // an inbox race to consume each other's replies. Per binding it is
        // fine, since a binding belongs to one workload. There is no safe
        // reading of it here, so it is refused rather than warned about.
        if self.base.contains_key("inbox-prefix") {
            anyhow::bail!(
                "`inbox-prefix` cannot be set on the host-wide wasmcloud:nats config: it would \
                 give every workload on this host the same inbox, and two workloads sharing an \
                 inbox consume each other's replies. Set it on a single named binding, or leave \
                 it unset — the per-workload default already isolates replies"
            )
        }

        // The base alone is not required to be complete: a host that sets only
        // grants leaves the servers to a workload under `Allow`.
        for name in self.bindings.keys() {
            let layer = self.host_layer(name);
            NatsConfig::from_map(&layer)
                .map_err(|e| anyhow::anyhow!("host.wasmcloudNats binding `{name}`: {e:#}"))?;
        }
        Ok(())
    }
}

/// A binding name for a log line or an error message.
fn describe(binding: &str) -> &str {
    if binding.is_empty() {
        "<unnamed>"
    } else {
        binding
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// The common case: a workload on the cluster's own NATS names no servers
    /// and inherits the host's address.
    #[test]
    fn a_binding_without_servers_falls_back_to_the_host() {
        let bindings = NatsBindings::new().with_default_servers(vec!["nats://host:4222".into()]);
        let resolved = bindings
            .resolve(UNNAMED_BINDING, map(&[("subject-allow", "orders.>")]))
            .expect("the host default satisfies the binding");
        assert_eq!(
            resolved.get("servers").map(String::as_str),
            Some("nats://host:4222")
        );
    }

    /// Under `Allow` a manifest still describes its own binding, so a project
    /// that runs under `wash dev` keeps working.
    #[test]
    fn allow_lets_a_workload_override_the_host() {
        let bindings = NatsBindings::new().with_default_servers(vec!["nats://host:4222".into()]);
        let resolved = bindings
            .resolve(
                UNNAMED_BINDING,
                map(&[("servers", "nats://elsewhere:4222")]),
            )
            .expect("allow accepts a workload's own servers");
        assert_eq!(
            resolved.get("servers").map(String::as_str),
            Some("nats://elsewhere:4222"),
            "the binding's own server replaces the default rather than merging with it"
        );
    }

    /// The address carries no reach: inheriting the host's servers grants
    /// nothing on its own.
    #[test]
    fn the_host_default_carries_no_grant() {
        let bindings = NatsBindings::new().with_default_servers(vec!["nats://host:4222".into()]);
        let resolved = bindings.resolve(UNNAMED_BINDING, HashMap::new()).unwrap();
        for key in GRANT_KEYS {
            assert!(
                !resolved.contains_key(*key),
                "`{key}` must not be inherited"
            );
        }
    }

    /// Under `Deny` the grants come from the host's declaration, and the
    /// workload supplies none.
    #[test]
    fn deny_serves_the_hosts_declaration() {
        let bindings = NatsBindings::new()
            .with_workload_config(WorkloadConfig::Deny)
            .with_default_servers(vec!["nats://host:4222".into()])
            .with_binding(
                "orders",
                map(&[
                    ("subject-allow", "orders.processed,orders.received"),
                    ("stream-allow", "ORDERS,PROCESSED"),
                    ("bucket-allow", "order-totals"),
                ]),
            );

        let resolved = bindings
            .resolve(
                "orders",
                map(&[("subscriptions", "ORDERS:orders.received:all")]),
            )
            .expect("a workload that only asks is accepted");

        assert_eq!(
            resolved.get("servers").map(String::as_str),
            Some("nats://host:4222")
        );
        assert_eq!(
            resolved.get("subject-allow").map(String::as_str),
            Some("orders.processed,orders.received")
        );
        assert_eq!(
            resolved.get("subscriptions").map(String::as_str),
            Some("ORDERS:orders.received:all"),
            "what the workload receives is still the workload's to declare"
        );
    }

    /// The address flag is a fallback: an operator who wrote `servers` into the
    /// host's own configuration meant that address.
    #[test]
    fn the_default_address_does_not_clobber_the_operators() {
        let bindings = NatsBindings::new()
            .with_base(map(&[("servers", "nats://declared:4222")]))
            .with_default_servers(vec!["nats://flag:4222".into()]);
        let resolved = bindings.resolve(UNNAMED_BINDING, HashMap::new()).unwrap();
        assert_eq!(
            resolved.get("servers").map(String::as_str),
            Some("nats://declared:4222")
        );
    }

    /// A named binding layers over the base rather than replacing it.
    #[test]
    fn a_named_binding_layers_over_the_base() {
        let bindings = NatsBindings::new()
            .with_workload_config(WorkloadConfig::Deny)
            .with_base(map(&[
                ("servers", "nats://host:4222"),
                ("creds", "/etc/base.creds"),
            ]))
            .with_binding("orders", map(&[("creds", "/etc/orders.creds")]));

        let resolved = bindings.resolve("orders", HashMap::new()).unwrap();
        assert_eq!(
            resolved.get("servers").map(String::as_str),
            Some("nats://host:4222")
        );
        assert_eq!(
            resolved.get("creds").map(String::as_str),
            Some("/etc/orders.creds"),
            "the named entry wins over the base"
        );
    }

    /// A workload cannot widen its own grant.
    #[test]
    fn deny_refuses_a_workloads_grant() {
        let bindings = NatsBindings::new()
            .with_workload_config(WorkloadConfig::Deny)
            .with_default_servers(vec!["nats://host:4222".into()])
            .with_binding("orders", map(&[("subject-allow", "orders.processed")]));

        let err = bindings
            .resolve("orders", map(&[("subject-allow", "orders.>")]))
            .expect_err("a manifest may not grant itself subjects");
        let msg = err.to_string();
        assert!(
            msg.contains("`subject-allow`"),
            "names the refused key: {msg}"
        );
    }

    /// Nor point itself at another cluster, nor connect as someone else.
    #[test]
    fn deny_refuses_a_workloads_connection_settings() {
        let bindings = NatsBindings::new()
            .with_workload_config(WorkloadConfig::Deny)
            .with_default_servers(vec!["nats://host:4222".into()]);

        for key in [
            "servers",
            "creds",
            "token",
            "nkey-seed",
            "tls-ca",
            "inbox-prefix",
        ] {
            bindings
                .resolve(UNNAMED_BINDING, map(&[(key, "value")]))
                .unwrap_err();
        }
    }

    /// Naming a binding the host does not serve is a deployment error, not an
    /// empty grant discovered one denied call at a time.
    #[test]
    fn deny_refuses_an_undeclared_binding() {
        let bindings = NatsBindings::new()
            .with_workload_config(WorkloadConfig::Deny)
            .with_default_servers(vec!["nats://host:4222".into()])
            .with_binding("orders", map(&[("subject-allow", "orders.>")]));

        let err = bindings
            .resolve("shipments", HashMap::new())
            .expect_err("the host serves no `shipments`");
        let msg = err.to_string();
        assert!(
            msg.contains("shipments"),
            "names the binding asked for: {msg}"
        );
        assert!(msg.contains("orders"), "names what it does serve: {msg}");
    }

    /// A snake_case spelling is the same key, so denying `subject-allow` denies
    /// `subject_allow` too.
    #[test]
    fn deny_reads_both_spellings() {
        let bindings = NatsBindings::new()
            .with_workload_config(WorkloadConfig::Deny)
            .with_default_servers(vec!["nats://host:4222".into()]);

        bindings
            .resolve(UNNAMED_BINDING, map(&[("subject_allow", "orders.>")]))
            .expect_err("the snake_case spelling is the same grant");
    }

    /// A host-wide `inbox-prefix` would hand every workload the same inbox, so
    /// it is refused where it is written.
    #[test]
    fn validate_refuses_a_host_wide_inbox_prefix() {
        let err = NatsBindings::new()
            .with_base(map(&[
                ("servers", "nats://host:4222"),
                ("inbox-prefix", "_INBOX_shared"),
            ]))
            .validate()
            .expect_err("a host-wide inbox prefix must be refused");
        assert!(
            err.to_string().contains("inbox-prefix"),
            "names the key: {err:#}"
        );

        // Per binding it is safe: a binding belongs to one workload.
        NatsBindings::new()
            .with_base(map(&[("servers", "nats://host:4222")]))
            .with_binding("orders", map(&[("inbox-prefix", "_INBOX_orders")]))
            .validate()
            .expect("a named binding may set its own inbox prefix");
    }

    /// A declared binding that cannot be parsed fails at startup, not at the
    /// first workload that asks for it.
    #[test]
    fn validate_rejects_a_bad_declaration() {
        let err = NatsBindings::new()
            .with_default_servers(vec!["nats://host:4222".into()])
            .with_binding("orders", map(&[("ack-mode", "sometimes")]))
            .validate()
            .expect_err("`sometimes` is not an ack mode");
        assert!(
            err.to_string().contains("orders"),
            "names the binding: {err:#}"
        );
    }

    /// A binding declared on a host with no address is refused at startup, for
    /// the same reason.
    #[test]
    fn validate_rejects_a_binding_with_no_servers() {
        NatsBindings::new()
            .with_binding("orders", map(&[("subject-allow", "orders.>")]))
            .validate()
            .expect_err("a declared binding needs an address");
    }
}
