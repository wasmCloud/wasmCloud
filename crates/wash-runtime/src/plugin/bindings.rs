//! Operator-declared plugin bindings: the host's side of every
//! `interface-binding{name, config}` a workload can ask for.
//!
//! A workload names a binding and supplies config for it; an operator declares
//! what that name *is*. [`PluginBindings`] is the catalog of those
//! declarations, one [`PluginBindingSet`] per plugin id, and
//! [`PluginBindingSet::resolve_interfaces`] is where the two meet: the host's
//! layer underneath, the workload's own config on top, and — under
//! [`WorkloadConfigPolicy::Deny`] — a refusal for any key the host owns.
//!
//! This is deliberately plugin-agnostic. A plugin contributes only a
//! [`BindingSchema`] naming the keys it considers the host's, and never sees
//! the difference: by the time [`crate::plugin::HostPlugin::on_workload_bind`]
//! runs, each interface's `config` is already the merged, policy-checked map.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

use crate::wit::WitInterface;

/// The binding name of a plain, unlabeled import.
pub const UNNAMED_BINDING: &str = "";

/// Canonical spelling of a config key: kebab-case.
///
/// Config keys reach a plugin in whichever spelling a manifest used, and most
/// plugin readers accept both. Comparing raw keys would let `subject_allow`
/// slip past a host-owned check written as `subject-allow`, so every comparison
/// in this module goes through here first.
#[must_use]
pub fn canonical_key(key: &str) -> String {
    key.trim().replace('_', "-")
}

/// Who supplies the config for a plugin's bindings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkloadConfigPolicy {
    /// A workload's own `interface-binding` config supplies it, with the
    /// operator's declaration as a default beneath it.
    ///
    /// The default, and what every plugin got before this existed: denying keys
    /// nothing else supplies would leave a binding unusable.
    #[default]
    Allow,
    /// Host-owned keys come only from the operator. A workload that sets one is
    /// refused at bind, and a workload that names a binding the operator did
    /// not declare is refused rather than handed the base config.
    Deny,
}

impl WorkloadConfigPolicy {
    /// The spelling used in config files and on the CLI.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
        }
    }

    /// Whether this is the default, for `skip_serializing_if`.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl std::fmt::Display for WorkloadConfigPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for WorkloadConfigPolicy {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "allow" => Ok(Self::Allow),
            "deny" => Ok(Self::Deny),
            other => anyhow::bail!("unknown workloadConfig {other:?}; expected `allow` or `deny`"),
        }
    }
}

/// What a plugin declares about its own config keys.
///
/// A plugin compiles in the keys that decide where a binding connects, as whom,
/// and what it may reach — the ones an operator running under
/// [`WorkloadConfigPolicy::Deny`] must own even when nothing set them. Everything
/// else (a subscription list, a timeout) stays the workload's to write.
///
/// List every alias the plugin's own reader accepts. A deny set that names only
/// one spelling of a key the reader takes under two denies nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BindingSchema {
    host_owned: BTreeSet<String>,
}

impl BindingSchema {
    /// A schema that owns no keys — every key is the workload's to write.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// A schema owning `keys`, canonicalized.
    pub fn with_host_owned_keys<I, S>(keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            host_owned: keys
                .into_iter()
                .map(|k| canonical_key(k.as_ref()))
                .collect(),
        }
    }

    /// The keys this plugin considers the host's, in canonical spelling.
    pub fn host_owned_keys(&self) -> impl Iterator<Item = &str> {
        self.host_owned.iter().map(String::as_str)
    }

    /// Whether the plugin itself declares `key` (any spelling) host-owned.
    #[must_use]
    pub fn owns(&self, key: &str) -> bool {
        self.host_owned.contains(&canonical_key(key))
    }

    /// Whether this schema names nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.host_owned.is_empty()
    }
}

/// One plugin's operator declaration: a base config, the named bindings it
/// serves, and who may configure them.
///
/// Stored as the plain string maps a manifest writes, so an operator's config
/// and a workload's go through exactly the same plugin parser — an operator
/// cannot write a value the manifest parser would have rejected.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginBindingSet {
    plugin_id: String,
    base: HashMap<String, String>,
    bindings: BTreeMap<String, HashMap<String, String>>,
    declared_host_owned: BTreeSet<String>,
    workload_config: WorkloadConfigPolicy,
}

/// Handed back by [`PluginBindings::for_plugin`] for a plugin no operator
/// declared anything for: allow-all, no host layer, so resolution is the
/// workload's own config unchanged.
static UNDECLARED: LazyLock<PluginBindingSet> = LazyLock::new(PluginBindingSet::default);

impl PluginBindingSet {
    /// An empty declaration for `plugin_id`.
    pub fn new(plugin_id: impl Into<String>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            ..Self::default()
        }
    }

    /// Config applied to every binding of this plugin, named or not.
    #[must_use]
    pub fn with_base(mut self, config: HashMap<String, String>) -> Self {
        self.base = config;
        self
    }

    /// Declare the binding `name`, with config layered over the base.
    #[must_use]
    pub fn with_binding(
        mut self,
        name: impl Into<String>,
        config: HashMap<String, String>,
    ) -> Self {
        self.bindings.insert(name.into(), config);
        self
    }

    /// Keys the operator claims for the host even though nothing here sets
    /// them — the `hostOwnedKeys` list.
    #[must_use]
    pub fn with_host_owned_keys<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.declared_host_owned
            .extend(keys.into_iter().map(|k| canonical_key(k.as_ref())));
        self
    }

    /// Whether a workload may write host-owned keys for this plugin.
    #[must_use]
    pub fn with_workload_config(mut self, workload_config: WorkloadConfigPolicy) -> Self {
        self.workload_config = workload_config;
        self
    }

    /// Seed `key` on the base layer only if the operator did not set it — how a
    /// CLI flag supplies a fallback without overriding the config file.
    #[must_use]
    pub fn with_default(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.base
            .entry(canonical_key(&key.into()))
            .or_insert_with(|| value.into());
        self
    }

    /// The plugin this declaration is for.
    #[must_use]
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Who may configure this plugin's bindings.
    #[must_use]
    pub fn workload_config(&self) -> WorkloadConfigPolicy {
        self.workload_config
    }

    /// The names the operator declared, sorted.
    pub fn binding_names(&self) -> impl Iterator<Item = &str> {
        self.bindings.keys().map(String::as_str)
    }

    /// The operator's config for `binding`: the base with the named entry
    /// layered over it.
    #[must_use]
    pub fn host_layer(&self, binding: &str) -> HashMap<String, String> {
        let mut layer = self.base.clone();
        if let Some(named) = self.bindings.get(binding) {
            layer.extend(named.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        layer
    }

    /// Every key the host owns for this plugin, in canonical spelling:
    ///
    /// ```text
    /// schema.host_owned_keys()   // compiled into the plugin
    ///   ∪ entry.hostOwnedKeys    // declared in config, even when unset
    ///   ∪ keys the operator set  // config + bindings.*.config
    /// ```
    ///
    /// The third term is what makes an operator's own declaration binding: a
    /// key they wrote anywhere for this plugin is theirs, on every binding.
    #[must_use]
    pub fn effective_host_owned(&self, schema: &BindingSchema) -> BTreeSet<String> {
        let mut owned: BTreeSet<String> = schema.host_owned.clone();
        owned.extend(self.declared_host_owned.iter().cloned());
        owned.extend(self.base.keys().map(|k| canonical_key(k)));
        for config in self.bindings.values() {
            owned.extend(config.keys().map(|k| canonical_key(k)));
        }
        owned
    }

    /// Whether resolution can hand back the workload's config untouched.
    fn is_passthrough(&self, schema: &BindingSchema) -> bool {
        self.workload_config == WorkloadConfigPolicy::Allow
            && self.base.is_empty()
            && self.bindings.is_empty()
            && schema.is_empty()
            && self.declared_host_owned.is_empty()
    }

    /// The binding name an interface routes under: its `(implements ..)` label,
    /// or the unnamed binding.
    ///
    /// A label equal to the plugin's own id is how a workload routes directly
    /// to a plugin rather than naming a backend, so it reads as unnamed here —
    /// no operator declares `bindings.<plugin-id>` under that same plugin.
    fn binding_name<'a>(&self, interface: &'a WitInterface) -> &'a str {
        match interface.name.as_deref() {
            Some(name) if name != self.plugin_id => name,
            _ => UNNAMED_BINDING,
        }
    }

    /// Merge the operator's declaration for `binding` with what a workload
    /// wrote, refusing under [`WorkloadConfigPolicy::Deny`] what the host owns.
    ///
    /// Precedence is host layer, then workload config — last wins — compared by
    /// canonical key, so a workload's `subject_allow` replaces the operator's
    /// `subject-allow` rather than sitting beside it. Under `Deny` no host-owned
    /// key survives the check, so that ordering only ever applies to keys the
    /// workload is entitled to write.
    ///
    /// # Errors
    ///
    /// Under `Deny`: a `binding` name the operator did not declare, or a
    /// workload config that sets a host-owned key.
    pub fn resolve(
        &self,
        binding: &str,
        workload: &HashMap<String, String>,
        schema: &BindingSchema,
    ) -> anyhow::Result<HashMap<String, String>> {
        if self.workload_config == WorkloadConfigPolicy::Deny {
            // Falling back to the base config for an undeclared name would start
            // the workload against the right backend with none of the grants the
            // operator meant it to have, and every call would be refused one at
            // a time with nothing pointing at the missing declaration.
            if binding != UNNAMED_BINDING && !self.bindings.contains_key(binding) {
                anyhow::bail!(
                    "plugin `{}` serves no binding named `{binding}`{}. A workload asks for a \
                     binding by name and the host declares what it is; add `{binding}` under \
                     `host.plugins` entry `{}`'s `bindings` in the host's config file",
                    self.plugin_id,
                    self.describe_available(),
                    self.plugin_id,
                );
            }

            let owned = self.effective_host_owned(schema);
            let mut refused: Vec<String> = workload
                .keys()
                .filter(|key| owned.contains(&canonical_key(key)))
                .map(|key| format!("`{key}`"))
                .collect();
            if !refused.is_empty() {
                // Sorted: `workload` is a `HashMap`, and the same manifest has
                // to be refused naming the same key every time.
                refused.sort();
                anyhow::bail!(
                    "binding `{}` of plugin `{}` sets {}, which this host does not accept from a \
                     workload. Those keys belong to the operator under `workloadConfig: deny` — \
                     a manifest that set them could point itself at another backend or widen its \
                     own grant. Ask the operator to declare them under `host.plugins`",
                    describe_binding(binding),
                    self.plugin_id,
                    refused.join(", "),
                );
            }
        }

        let mut resolved = self.host_layer(binding);
        for (key, value) in workload {
            let canonical = canonical_key(key);
            // Replace the operator's spelling rather than adding beside it, so
            // one key stays one key however each side spelled it.
            resolved.retain(|existing, _| canonical_key(existing) != canonical);
            resolved.insert(key.clone(), value.clone());
        }
        Ok(resolved)
    }

    /// Resolve every interface a plugin matched, replacing each one's config
    /// with the merged, policy-checked map.
    ///
    /// # Errors
    ///
    /// The first interface [`PluginBindingSet::resolve`] refuses. Interfaces are
    /// ordered before resolution, so a manifest that cannot be deployed is
    /// refused with the same message every time rather than with whichever
    /// entry the set happened to yield first.
    pub fn resolve_interfaces(
        &self,
        interfaces: &HashSet<WitInterface>,
        schema: &BindingSchema,
    ) -> anyhow::Result<HashSet<WitInterface>> {
        if self.is_passthrough(schema) {
            return Ok(interfaces.clone());
        }

        let mut ordered: Vec<&WitInterface> = interfaces.iter().collect();
        ordered.sort_by_cached_key(|interface| {
            let mut keys: Vec<&str> = interface.config.keys().map(String::as_str).collect();
            keys.sort_unstable();
            (interface.instance(), keys.join(",").to_string())
        });

        let mut resolved = HashSet::with_capacity(interfaces.len());
        for interface in ordered {
            let binding = self.binding_name(interface);
            let mut merged = interface.clone();
            merged.config = self.resolve(binding, &interface.config, schema)?;
            resolved.insert(merged);
        }
        Ok(resolved)
    }

    /// Every named binding with the base layered underneath — what a plugin's
    /// own parser checks at startup so a bad declaration fails the host rather
    /// than the first call that needs it.
    ///
    /// The base alone is not included: it is not required to be a complete
    /// configuration on its own, since under [`WorkloadConfigPolicy::Allow`] a
    /// workload supplies the rest.
    pub fn host_layers(&self) -> impl Iterator<Item = (&str, HashMap<String, String>)> {
        self.bindings
            .keys()
            .map(|name| (name.as_str(), self.host_layer(name)))
    }

    /// The base layer on its own, for a plugin checking a key that is only
    /// wrong host-wide (one inbox prefix shared by every workload, say).
    #[must_use]
    pub fn base(&self) -> &HashMap<String, String> {
        &self.base
    }

    /// `; this host serves ...` for an error, or nothing when it serves none.
    fn describe_available(&self) -> String {
        if self.bindings.is_empty() {
            return String::new();
        }
        let names: Vec<String> = self.bindings.keys().map(|n| format!("`{n}`")).collect();
        format!("; it serves {}", names.join(", "))
    }
}

/// How a binding reads in an error message.
fn describe_binding(binding: &str) -> &str {
    if binding == UNNAMED_BINDING {
        "<unnamed>"
    } else {
        binding
    }
}

/// Every operator-declared [`PluginBindingSet`], by plugin id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginBindings {
    plugins: BTreeMap<String, PluginBindingSet>,
}

impl PluginBindings {
    /// A catalog with nothing declared: every plugin resolves passthrough.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare `set` for the plugin it names, replacing any earlier entry.
    #[must_use]
    pub fn with_plugin(mut self, set: PluginBindingSet) -> Self {
        self.plugins.insert(set.plugin_id.clone(), set);
        self
    }

    /// Whether nothing is declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// The plugin ids something is declared for, sorted.
    pub fn plugin_ids(&self) -> impl Iterator<Item = &str> {
        self.plugins.keys().map(String::as_str)
    }

    /// The declaration for `plugin_id`, or an allow-all empty one.
    #[must_use]
    pub fn for_plugin(&self, plugin_id: &str) -> &PluginBindingSet {
        self.plugins.get(plugin_id).unwrap_or(&UNDECLARED)
    }

    /// Refuse a declaration naming a plugin this host does not have.
    ///
    /// A typo'd plugin id would otherwise be inert: bindings declared and
    /// nothing consuming them, including a `workloadConfig: deny` that never
    /// takes effect.
    ///
    /// # Errors
    ///
    /// Names every declared id absent from `registered`.
    pub fn validate_against(&self, registered: &[&str]) -> anyhow::Result<()> {
        let unknown: Vec<&str> = self
            .plugin_ids()
            .filter(|id| !registered.contains(id))
            .collect();
        if unknown.is_empty() {
            return Ok(());
        }
        let mut known: Vec<&str> = registered.to_vec();
        known.sort_unstable();
        anyhow::bail!(
            "`host.plugins` declares {}, which this host has no plugin for. Registered plugins: \
             {}",
            unknown
                .iter()
                .map(|id| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(", "),
            known.join(", "),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn iface(name: Option<&str>, config: &[(&str, &str)]) -> WitInterface {
        WitInterface {
            namespace: "wasmcloud".into(),
            package: "nats".into(),
            interfaces: ["core"].into_iter().map(str::to_string).collect(),
            version: None,
            config: map(config),
            name: name.map(str::to_string),
        }
    }

    fn nats_schema() -> BindingSchema {
        BindingSchema::with_host_owned_keys(["servers", "creds", "subject-allow"])
    }

    #[test]
    fn undeclared_plugin_resolves_passthrough() {
        let bindings = PluginBindings::new();
        let set = bindings.for_plugin("wasmcloud-nats");
        let interfaces: HashSet<_> = [iface(None, &[("servers", "nats://guest:4222")])].into();

        let resolved = set
            .resolve_interfaces(&interfaces, &BindingSchema::empty())
            .unwrap();
        assert_eq!(resolved, interfaces);
    }

    #[test]
    fn allow_layers_workload_over_the_operator() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_base(map(&[
                ("servers", "nats://host:4222"),
                ("creds", "/h.creds"),
            ]))
            .with_binding("orders", map(&[("subject-allow", "orders.>")]));

        let resolved = set
            .resolve(
                "orders",
                &map(&[("servers", "nats://guest:4222"), ("ack-mode", "manual")]),
                &nats_schema(),
            )
            .unwrap();

        assert_eq!(resolved.get("servers").unwrap(), "nats://guest:4222");
        assert_eq!(resolved.get("creds").unwrap(), "/h.creds");
        assert_eq!(resolved.get("subject-allow").unwrap(), "orders.>");
        assert_eq!(resolved.get("ack-mode").unwrap(), "manual");
    }

    #[test]
    fn a_workload_key_replaces_the_operators_other_spelling() {
        // Not both spellings side by side: one key stays one key, whichever way
        // each side wrote it.
        let set = PluginBindingSet::new("p").with_base(map(&[("ack_mode", "auto")]));
        let resolved = set
            .resolve("", &map(&[("ack-mode", "manual")]), &BindingSchema::empty())
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved.get("ack-mode").unwrap(), "manual");
    }

    #[test]
    fn deny_refuses_a_host_owned_key_in_either_spelling() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_binding("orders", map(&[("subject-allow", "orders.>")]))
            .with_workload_config(WorkloadConfigPolicy::Deny);

        for spelling in ["subject-allow", "subject_allow"] {
            let err = set
                .resolve("orders", &map(&[(spelling, ">")]), &nats_schema())
                .unwrap_err()
                .to_string();
            assert!(err.contains(spelling), "got: {err}");
            assert!(
                err.contains("does not accept from a workload"),
                "got: {err}"
            );
        }
    }

    #[test]
    fn deny_leaves_workload_owned_keys_alone() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_base(map(&[("servers", "nats://host:4222")]))
            .with_binding("orders", map(&[("subject-allow", "orders.>")]))
            .with_workload_config(WorkloadConfigPolicy::Deny);

        let resolved = set
            .resolve(
                "orders",
                &map(&[("core-subscriptions", "orders.received")]),
                &nats_schema(),
            )
            .unwrap();
        assert_eq!(resolved.get("servers").unwrap(), "nats://host:4222");
        assert_eq!(resolved.get("subject-allow").unwrap(), "orders.>");
        assert_eq!(
            resolved.get("core-subscriptions").unwrap(),
            "orders.received"
        );
    }

    #[test]
    fn deny_refuses_an_undeclared_binding_name_and_lists_what_is_served() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_binding("orders", map(&[("subject-allow", "orders.>")]))
            .with_workload_config(WorkloadConfigPolicy::Deny);

        let err = set
            .resolve("shipping", &HashMap::new(), &nats_schema())
            .unwrap_err()
            .to_string();
        assert!(err.contains("`shipping`"), "got: {err}");
        assert!(err.contains("it serves `orders`"), "got: {err}");
    }

    #[test]
    fn deny_still_serves_the_unnamed_binding() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_base(map(&[("servers", "nats://host:4222")]))
            .with_workload_config(WorkloadConfigPolicy::Deny);
        let resolved = set.resolve("", &HashMap::new(), &nats_schema()).unwrap();
        assert_eq!(resolved.get("servers").unwrap(), "nats://host:4222");
    }

    #[test]
    fn effective_host_owned_is_the_union_of_all_three_sources() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_base(map(&[("jetstream-domain", "hub")]))
            .with_binding("orders", map(&[("stream_allow", "ORDERS")]))
            .with_host_owned_keys(["inbox-prefix"]);

        let owned = set.effective_host_owned(&nats_schema());
        // compiled in
        assert!(owned.contains("servers"));
        assert!(owned.contains("subject-allow"));
        // declared unset
        assert!(owned.contains("inbox-prefix"));
        // actually set, canonicalized from the base and from a binding
        assert!(owned.contains("jetstream-domain"));
        assert!(owned.contains("stream-allow"));
        assert!(!owned.contains("ack-mode"));
    }

    #[test]
    fn an_unset_compiled_in_key_is_still_refused_under_deny() {
        // The whole point of `binding_schema`: `subject-allow` resolves to
        // empty rather than to whatever the manifest wrote.
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_workload_config(WorkloadConfigPolicy::Deny);
        let err = set
            .resolve("", &map(&[("subject-allow", ">")]), &nats_schema())
            .unwrap_err()
            .to_string();
        assert!(err.contains("subject-allow"), "got: {err}");
    }

    #[test]
    fn a_label_matching_the_plugin_id_routes_to_the_unnamed_binding() {
        // `(implements wasmcloud-nats)` is plugin-id routing, not a backend
        // name, so it takes the base config rather than being refused as
        // undeclared.
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_base(map(&[("servers", "nats://host:4222")]))
            .with_workload_config(WorkloadConfigPolicy::Deny);

        let interfaces: HashSet<_> = [iface(Some("wasmcloud-nats"), &[])].into();
        let resolved = set.resolve_interfaces(&interfaces, &nats_schema()).unwrap();
        let only = resolved.iter().next().unwrap();
        assert_eq!(only.config.get("servers").unwrap(), "nats://host:4222");
    }

    #[test]
    fn resolve_interfaces_resolves_each_label_against_its_own_binding() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_base(map(&[("servers", "nats://host:4222")]))
            .with_binding("orders", map(&[("subject-allow", "orders.>")]))
            .with_binding("shipping", map(&[("subject-allow", "shipping.>")]))
            .with_workload_config(WorkloadConfigPolicy::Deny);

        let interfaces: HashSet<_> = [
            iface(Some("orders"), &[("ack-mode", "manual")]),
            iface(Some("shipping"), &[]),
        ]
        .into();
        let resolved = set.resolve_interfaces(&interfaces, &nats_schema()).unwrap();

        let by_name: HashMap<&str, &HashMap<String, String>> = resolved
            .iter()
            .map(|i| (i.name.as_deref().unwrap(), &i.config))
            .collect();
        assert_eq!(by_name["orders"]["subject-allow"], "orders.>");
        assert_eq!(by_name["orders"]["ack-mode"], "manual");
        assert_eq!(by_name["shipping"]["subject-allow"], "shipping.>");
        assert_eq!(by_name["shipping"]["servers"], "nats://host:4222");
        assert!(!by_name["shipping"].contains_key("ack-mode"));
    }

    #[test]
    fn a_refusal_names_the_same_key_every_time() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_workload_config(WorkloadConfigPolicy::Deny);
        let workload = map(&[("subject-allow", ">"), ("servers", "nats://x:4222")]);
        let first = set
            .resolve("", &workload, &nats_schema())
            .unwrap_err()
            .to_string();
        for _ in 0..16 {
            assert_eq!(
                set.resolve("", &workload, &nats_schema())
                    .unwrap_err()
                    .to_string(),
                first
            );
        }
        // Sorted, so `servers` leads.
        assert!(first.contains("`servers`, `subject-allow`"), "got: {first}");
    }

    #[test]
    fn with_default_does_not_override_the_operator() {
        let set = PluginBindingSet::new("p")
            .with_base(map(&[("servers", "nats://operator:4222")]))
            .with_default("servers", "nats://flag:4222")
            .with_default("name", "wash");
        assert_eq!(set.host_layer("")["servers"], "nats://operator:4222");
        assert_eq!(set.host_layer("")["name"], "wash");
    }

    #[test]
    fn host_layers_yields_each_named_binding_over_the_base() {
        let set = PluginBindingSet::new("kv")
            .with_base(map(&[("url", "redis://host:6379")]))
            .with_binding("cache", map(&[("bucket", "cache")]))
            .with_binding("sessions", map(&[("url", "redis://other:6379")]));

        let layers: BTreeMap<&str, HashMap<String, String>> = set.host_layers().collect();
        assert_eq!(layers["cache"]["url"], "redis://host:6379");
        assert_eq!(layers["cache"]["bucket"], "cache");
        assert_eq!(layers["sessions"]["url"], "redis://other:6379");
        // The base alone is not a layer: it need not be complete on its own.
        assert_eq!(layers.len(), 2);
        assert_eq!(set.base()["url"], "redis://host:6379");
    }

    #[test]
    fn validate_against_names_the_unknown_id() {
        let bindings = PluginBindings::new().with_plugin(PluginBindingSet::new("wasmcloud-nat"));
        let err = bindings
            .validate_against(&["wasmcloud-nats", "wasi-keyvalue"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("`wasmcloud-nat`"), "got: {err}");
        assert!(err.contains("wasmcloud-nats"), "got: {err}");

        PluginBindings::new()
            .with_plugin(PluginBindingSet::new("wasmcloud-nats"))
            .validate_against(&["wasmcloud-nats"])
            .unwrap();
    }

    #[test]
    fn workload_config_policy_round_trips_through_its_spelling() {
        for policy in [WorkloadConfigPolicy::Allow, WorkloadConfigPolicy::Deny] {
            assert_eq!(
                policy.as_str().parse::<WorkloadConfigPolicy>().unwrap(),
                policy
            );
        }
        assert!("sometimes".parse::<WorkloadConfigPolicy>().is_err());
    }
}
