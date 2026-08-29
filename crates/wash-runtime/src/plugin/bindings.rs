//! Operator-declared plugin bindings: the host's side of every
//! `interface-binding{name, config}` a workload can ask for.
//!
//! A workload names a binding and supplies config for it; an operator declares
//! what that name *is*. [`PluginBindings`] is the catalog of those
//! declarations, one [`PluginBindingSet`] per plugin id, and
//! [`PluginBindingSet::resolve_by_name`] is where the two meet: the workload's
//! own entries folded together, the host's layer underneath, and — under
//! [`WorkloadConfigPolicy::Deny`] — a refusal for anything the host owns.
//!
//! This is deliberately plugin-agnostic. A plugin contributes a
//! [`BindingSchema`] classifying its keys, and a containment predicate for the
//! ones it marks narrowable. It never sees the difference: by the time
//! [`crate::plugin::HostPlugin::on_workload_bind`] runs, each interface's
//! `config` is already the merged, policy-checked map.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

use tracing::{info, warn};

use crate::wit::WitInterface;

/// The binding name of a plain, unlabeled import.
pub const UNNAMED_BINDING: &str = "";

/// Canonical spelling of a config key: trimmed, kebab-case, lower-case.
///
/// Every comparison in this module goes through here, so `Subject_Allow` and
/// `subject-allow` are one key to the ownership check, the schema, and the
/// merge alike.
///
/// A plugin's own reader looks a key up its own way, and the ownership check compares canonical forms; the two agree today
/// only because every reader happens to be exact-match and lower-case. The
/// first reader that folds case would otherwise make `Deny` bypassable — a
/// workload writes `Subject-Allow`, the ownership check does not recognise it,
/// the reader does. Folding here makes that agreement structural.
///
/// `to_ascii_lowercase` rather than `to_lowercase`: config keys are ASCII, and
/// Unicode casing has the Turkish-İ problem.
#[must_use]
pub fn canonical_key(key: &str) -> String {
    key.trim().replace('_', "-").to_ascii_lowercase()
}

/// A predicate deciding whether a workload's value for a narrowable key is
/// contained by the host's.
///
/// Supplied by the plugin — see [`crate::plugin::HostPlugin::narrows`]. This
/// module has no opinion about what containment means for any key.
pub type NarrowsFn<'a> = &'a dyn Fn(&str, &str, &str) -> bool;

/// A containment predicate that permits nothing, for a schema with no
/// narrowable keys.
pub fn never_narrows() -> NarrowsFn<'static> {
    &|_key: &str, _host: &str, _workload: &str| false
}

/// Who supplies the config for a plugin's bindings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkloadConfigPolicy {
    /// A workload's own `interface-binding` config supplies it, with the
    /// operator's declaration as a default beneath it.
    Allow,
    /// Host-owned keys come only from the operator. A workload that sets one is
    /// refused at bind, and — once the operator has declared any binding for
    /// this plugin — a workload naming a binding absent from that declaration
    /// is refused rather than handed the base config.
    ///
    /// The default. It costs nothing where nothing is declared: with no
    /// [`BindingSchema`] and no operator config, the host owns no keys and
    /// claims no binding names, so `Deny` refuses exactly what `Allow` does
    /// (nothing). Strictness arrives the moment a plugin names its own keys or
    /// an operator writes a `host.plugins` entry — which is when it is wanted.
    #[default]
    Deny,
    /// `Allow` plus diagnostics: the workload's value still wins and nothing is
    /// refused, but everything [`WorkloadConfigPolicy::Deny`] *would* refuse is
    /// logged.
    ///
    /// A transition tool. Adopting `Deny` is a coordinated change between
    /// whoever writes manifests and whoever writes the host config — and
    /// sharper than it looks, since an operator with no declared bindings
    /// serves every label and declaring *one* refuses all the others. `Warn` is
    /// how that gets found before it bites.
    ///
    /// It deliberately does not change which value wins: the point is that
    /// flipping `Warn` → `Deny` is a no-op when the log is quiet, and a `Warn`
    /// that quietly preferred the host's value would be its own behavior change
    /// and prove nothing. It is defined as "everything `Deny` would refuse"
    /// rather than as a fixed list, so it stays correct however the refusal set
    /// grows.
    Warn,
}

impl WorkloadConfigPolicy {
    /// The spelling used in config files and on the CLI.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Warn => "warn",
        }
    }

    /// Whether this mode refuses. Only [`WorkloadConfigPolicy::Deny`] does.
    #[must_use]
    pub fn enforces(self) -> bool {
        self == Self::Deny
    }

    /// Whether this mode reports what `Deny` would refuse.
    #[must_use]
    pub fn reports(self) -> bool {
        self != Self::Allow
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
            "warn" => Ok(Self::Warn),
            other => anyhow::bail!(
                "unknown workloadConfig {other:?}; expected `allow`, `warn`, or `deny`"
            ),
        }
    }
}

/// Who a config key belongs to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyOwnership {
    /// The workload may set it freely. The default for any key a schema does
    /// not classify.
    #[default]
    Workload,
    /// The host's alone. A workload that sets it is refused under
    /// [`WorkloadConfigPolicy::Deny`] — an address or a credential has no
    /// meaningful "less".
    Host,
    /// The host declares a ceiling; a workload may set a value the host's
    /// value contains, and is refused if it does not narrow.
    ///
    /// For a grant list, refusing a workload that asked for *less* than it was
    /// given punishes least privilege in the right direction, which is the one
    /// thing a deny mode should never do. A workload that leaves the key unset
    /// takes the whole ceiling, so narrowing is opt-in and the default posture
    /// is still the maximum the operator declared.
    ///
    /// Inert under [`WorkloadConfigPolicy::Allow`], where the host layer is a
    /// default the workload overrides wholesale and no containment is checked.
    HostCeiling,
}

impl KeyOwnership {
    /// Whether the host has any claim on the key.
    #[must_use]
    pub fn is_hosts(self) -> bool {
        self != Self::Workload
    }
}

/// What a plugin declares about its own config keys: who each belongs to, and
/// — optionally — the complete set it reads at all.
///
/// [`KeyOwnership::Host`] and [`KeyOwnership::HostCeiling`] are the keys that
/// decide where a binding connects, as whom, and what it may reach: the ones an
/// operator running under [`WorkloadConfigPolicy::Deny`] must own even when
/// nothing set them. Everything else (a subscription list, a timeout) stays the
/// workload's to write.
///
/// Naming the workload's keys too closes the schema, and a closed schema is
/// checked: a key it does not classify at all is refused wherever it appears,
/// with the nearest spelling suggested. That is what keeps the classification
/// honest — a key added to the plugin's reader and forgotten here fails the
/// first manifest that uses it, loudly, instead of silently becoming
/// workload-writable under `Deny`. A schema that names only the host's keys
/// stays open, and unknown keys pass through.
///
/// List every alias the plugin's own reader accepts. A list naming one spelling
/// of a key the reader takes under two is wrong in both directions: it denies
/// nothing, and it refuses the other spelling as unknown. (Case, whitespace,
/// and `_` vs `-` need no separate entries — see [`canonical_key`].)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BindingSchema {
    ownership: BTreeMap<String, KeyOwnership>,
    closed: bool,
}

impl BindingSchema {
    /// A schema that classifies nothing — every key is the workload's to write,
    /// and nothing is refused as unknown.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// A schema whose `keys` are [`KeyOwnership::Host`]. Open until
    /// [`BindingSchema::and_workload_owned_keys`] closes it.
    pub fn with_host_owned_keys<I, S>(keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::default().classify(keys, KeyOwnership::Host)
    }

    /// Add [`KeyOwnership::HostCeiling`] keys: the host declares the maximum
    /// and a workload may take less. Does not close the schema.
    #[must_use]
    pub fn and_host_ceiling_keys<I, S>(self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.classify(keys, KeyOwnership::HostCeiling)
    }

    /// Name the remaining keys this plugin reads, closing the schema: from here
    /// on a key it does not classify is refused as unknown.
    ///
    /// Pass an empty iterator for a plugin whose every key is the host's — the
    /// closing is the point, not the contents.
    #[must_use]
    pub fn and_workload_owned_keys<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self = self.classify(keys, KeyOwnership::Workload);
        self.closed = true;
        self
    }

    fn classify<I, S>(mut self, keys: I, ownership: KeyOwnership) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for key in keys {
            self.ownership
                .insert(canonical_key(key.as_ref()), ownership);
        }
        self
    }

    /// How this plugin classifies `key` (any spelling).
    #[must_use]
    pub fn ownership(&self, key: &str) -> KeyOwnership {
        self.ownership
            .get(&canonical_key(key))
            .copied()
            .unwrap_or_default()
    }

    /// The keys the host has any claim on, in canonical spelling.
    pub fn host_owned_keys(&self) -> impl Iterator<Item = &str> {
        self.ownership
            .iter()
            .filter(|(_, o)| o.is_hosts())
            .map(|(k, _)| k.as_str())
    }

    /// Whether the plugin itself gives the host a claim on `key`.
    #[must_use]
    pub fn owns(&self, key: &str) -> bool {
        self.ownership(key).is_hosts()
    }

    /// Whether the plugin named every key it reads, so unknown keys are refused.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Whether this schema classifies nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ownership.is_empty() && !self.closed
    }

    /// Whether `key` (any spelling) is one this plugin reads.
    fn knows(&self, key: &str) -> bool {
        self.ownership.contains_key(&canonical_key(key))
    }

    /// Every key of `config` this plugin does not read, sorted, each with the
    /// closest known spelling where there is an obvious one.
    ///
    /// Empty on an open schema.
    fn unknown_keys(&self, config: &HashMap<String, String>) -> Vec<(String, Option<String>)> {
        if !self.closed {
            return Vec::new();
        }
        let mut unknown: Vec<(String, Option<String>)> = config
            .keys()
            .filter(|key| !self.knows(key))
            .map(|key| (key.clone(), self.nearest(key).map(str::to_string)))
            .collect();
        // Sorted: `config` is a `HashMap`, and the same input has to be
        // reported with the same message every time.
        unknown.sort();
        unknown
    }

    /// Refuse any key of `config` this plugin does not read, naming `owner` as
    /// where it was written.
    ///
    /// A no-op on an open schema.
    ///
    /// # Errors
    ///
    /// Names every unrecognized key, sorted, with the closest known spelling
    /// where there is an obvious one.
    pub fn reject_unknown_keys(
        &self,
        config: &HashMap<String, String>,
        owner: &str,
    ) -> anyhow::Result<()> {
        let unknown = self.unknown_keys(config);
        if unknown.is_empty() {
            return Ok(());
        }
        anyhow::bail!(
            "{owner} sets {}, which this plugin does not read. A key it does not recognize is \
             silently ignored, so a typo in a grant or a credential would leave the binding \
             configured as if nothing had been written",
            describe_unknown(&unknown),
        )
    }

    /// The known key closest to `key`, when one is close enough to be worth
    /// naming — a single edit away.
    fn nearest(&self, key: &str) -> Option<&str> {
        let key = canonical_key(key);
        self.ownership
            .keys()
            .find(|known| within_one_edit(&key, known))
            .map(String::as_str)
    }
}

/// `` `a` (did you mean `b`?), `c` `` for an unknown-key list.
fn describe_unknown(unknown: &[(String, Option<String>)]) -> String {
    unknown
        .iter()
        .map(|(key, near)| match near {
            Some(near) => format!("`{key}` (did you mean `{near}`?)"),
            None => format!("`{key}`"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether `a` and `b` differ by at most one insertion, deletion, or
/// substitution.
///
/// Enough to catch what a suggestion is for — a dropped letter (`subject-alow`)
/// or a fat-fingered one — without turning every unrelated key into a guess.
fn within_one_edit(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    if long.len() - short.len() > 1 {
        return false;
    }
    let same_length = short.len() == long.len();
    let (mut i, mut j, mut edited) = (0, 0, false);
    while let (Some(s), Some(l)) = (short.get(i), long.get(j)) {
        if s == l {
            i += 1;
            j += 1;
            continue;
        }
        if edited {
            return false;
        }
        edited = true;
        // A substitution consumes both; an insertion consumes only the longer.
        if same_length {
            i += 1;
        }
        j += 1;
    }
    true
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
    default_bundles: Vec<DefaultBundle>,
}

/// A set of defaults that applies only when nobody set its anchor key.
///
/// The case it exists for is TLS material, which is valid only for the address
/// it was issued for: an operator who points a binding at
/// `nats://external:4222` and sets no TLS must not inherit the *cluster's*
/// certs — a confusing handshake failure at best, and at worst a success
/// against something that accepts them.
///
/// Anchored on one key rather than on "any key in the bundle", so an operator
/// keeping the cluster address while bringing their own CA still gets the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DefaultBundle {
    anchor: String,
    entries: Vec<(String, String)>,
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

    /// Seed `entries` on every binding, but only where nobody set `anchor`.
    ///
    /// Unlike [`PluginBindingSet::with_default`], which is a construction-time
    /// seed into the base layer, a bundle is evaluated at **resolve time**.
    /// That is not a detail: under [`WorkloadConfigPolicy::Allow`] a *workload*
    /// can set the anchor, and a bundle seeded into the base at startup would
    /// already be sitting there when the workload's value lands — the same bug
    /// the anchor exists to prevent, one layer down.
    ///
    /// Within a bundle each entry is `or_insert`; the bundle as a whole is
    /// all-or-nothing on the anchor. A skipped bundle is logged, or an operator
    /// has no way to learn the rest of it did not come along.
    #[must_use]
    pub fn with_default_bundle<I, K, V>(mut self, anchor: impl AsRef<str>, entries: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: Into<String>,
    {
        let entries: Vec<(String, String)> = entries
            .into_iter()
            .map(|(k, v)| (canonical_key(k.as_ref()), v.into()))
            .collect();
        if !entries.is_empty() {
            self.default_bundles.push(DefaultBundle {
                anchor: canonical_key(anchor.as_ref()),
                entries,
            });
        }
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

    /// Every key the host has for this plugin, in canonical
    /// spelling:
    ///
    /// ```text
    /// schema.host_owned_keys()   // compiled into the plugin
    ///   ∪ entry.hostOwnedKeys    // declared in config, even when unset
    /// ```
    ///
    /// Both terms are explicit: **providing a value is
    /// not claiming a key.** An operator who writes `max-in-flight: 32` is
    /// setting a default a workload may still override; one who wants the key
    /// locked names it in `hostOwnedKeys`. Both cover keys left *unset* as
    /// well — a grant nobody declared has to resolve to empty, not to whatever
    /// the manifest wrote.
    #[must_use]
    pub fn effective_host_owned(&self, schema: &BindingSchema) -> BTreeSet<String> {
        schema
            .host_owned_keys()
            .map(str::to_string)
            .chain(self.declared_host_owned.iter().cloned())
            .collect()
    }

    /// How `key` resolves for this plugin, after the operator's own
    /// declaration.
    ///
    /// `hostOwnedKeys` upgrades a narrowable key to [`KeyOwnership::Host`]:
    /// naming a key explicitly reads as "mine", and it hands an operator a
    /// per-key opt-out of narrowing without needing a change to the plugin.
    #[must_use]
    pub fn ownership_of(&self, key: &str, schema: &BindingSchema) -> KeyOwnership {
        if self.declared_host_owned.contains(&canonical_key(key)) {
            return KeyOwnership::Host;
        }
        schema.ownership(key)
    }

    /// Whether resolution can hand back the workload's config untouched.
    ///
    /// Independent of the policy: `Deny` with nothing declared anywhere owns no
    /// keys, claims no names, and knows no schema to check against, so it
    /// refuses exactly what `Allow` would. That is what makes `Deny` a safe
    /// default for a host whose operator has written no `host.plugins` at all.
    fn is_passthrough(&self, schema: &BindingSchema) -> bool {
        self.base.is_empty()
            && self.bindings.is_empty()
            && schema.is_empty()
            && self.declared_host_owned.is_empty()
            && self.default_bundles.is_empty()
    }

    /// The binding name an interface routes under: its `(implements ..)` label,
    /// or the unnamed binding.
    ///
    /// A label equal to the plugin's own id is how a workload routes directly
    /// to a plugin rather than naming a backend, so it reads as unnamed here.
    fn binding_name<'a>(&self, interface: &'a WitInterface) -> &'a str {
        match interface.name.as_deref() {
            Some(name) if name != self.plugin_id => name,
            _ => UNNAMED_BINDING,
        }
    }

    /// Fold every entry a workload wrote for one binding into a single config.
    ///
    /// A binding is described by the manifest as a whole rather than by any one
    /// entry: the connection settings belong on the entry that imports the
    /// interface, while subscriptions belong on the entry that exports a
    /// handler. Reading each entry as a complete spec would refuse that split
    /// and leave authors copying the connection into every entry, where a later
    /// edit to one copy quietly gives one binding two configurations.
    ///
    /// This has to happen *before* the host layer lands. Applying the host
    /// layer per entry and folding afterwards makes an operator-set key that a
    /// workload overrides on one entry and not another come back differing
    /// between them, and be refused as a conflict the workload never wrote.
    ///
    /// # Errors
    ///
    /// Two entries of one binding setting the same key to different values.
    /// The entries open one connection checked against one grant, so a key they
    /// disagree about has no answer that can be picked here. The message names
    /// the key and not the values — they may be credentials.
    fn fold_workload_entries<'a>(
        &self,
        interfaces: &[&'a WitInterface],
    ) -> anyhow::Result<BTreeMap<&'a str, HashMap<String, String>>> {
        // Entries arrive out of a `HashSet`, so they are ordered before they
        // are folded: a manifest that cannot be deployed has to be refused with
        // the same key named every time.
        let mut ordered = interfaces.to_vec();
        ordered.sort_by_cached_key(|interface| {
            let mut keys: Vec<&str> = interface.config.keys().map(String::as_str).collect();
            keys.sort_unstable();
            (interface.instance(), keys.join(","))
        });

        let mut folded: BTreeMap<&str, HashMap<String, String>> = BTreeMap::new();
        for interface in ordered {
            let binding = self.binding_name(interface);
            let mut pairs: Vec<(&String, &String)> = interface.config.iter().collect();
            pairs.sort_by_key(|(key, _)| *key);

            let config = folded.entry(binding).or_default();
            for (key, value) in pairs {
                let canonical = canonical_key(key);
                if let Some(existing) = config.get(&canonical)
                    && existing != value
                {
                    anyhow::bail!(
                        "conflicting values for `{canonical}` across the entries of binding `{}` \
                         of plugin `{}`; the entries of one binding are folded into a single \
                         configuration, so a key more than one of them sets must agree",
                        describe_binding(binding),
                        self.plugin_id,
                    )
                }
                config.insert(canonical, value.clone());
            }
        }
        Ok(folded)
    }

    /// Resolve every binding a plugin matched: fold the workload's entries by
    /// label, then apply the host layer and the policy once per label.
    ///
    /// Keyed by label rather than by interface because one label is one
    /// binding — one connection, one grant — however many entries a manifest
    /// splits it across.
    ///
    /// # Errors
    ///
    /// A fold conflict, or — under [`WorkloadConfigPolicy::Deny`] — the first
    /// binding whose config the policy refuses. Bindings are resolved in name
    /// order, so a manifest that cannot be deployed is refused with the same
    /// message every time.
    pub fn resolve_by_name(
        &self,
        interfaces: &HashSet<WitInterface>,
        schema: &BindingSchema,
        narrows: NarrowsFn<'_>,
    ) -> anyhow::Result<BTreeMap<String, HashMap<String, String>>> {
        let borrowed: Vec<&WitInterface> = interfaces.iter().collect();
        let folded = self.fold_workload_entries(&borrowed)?;
        if self.is_passthrough(schema) {
            return Ok(folded
                .into_iter()
                .map(|(name, config)| (name.to_string(), config))
                .collect());
        }

        let mut resolved = BTreeMap::new();
        for (binding, workload) in folded {
            resolved.insert(
                binding.to_string(),
                self.resolve(binding, &workload, schema, narrows)?,
            );
        }
        Ok(resolved)
    }

    /// Stamp the configs `resolve_by_name` produced back onto `interfaces`.
    ///
    /// Every entry of one label carries the same folded, checked map, so a
    /// plugin reading `interface.config` in either bind callback sees the whole
    /// binding rather than the fragment that entry happened to declare.
    #[must_use]
    pub fn apply_resolved(
        &self,
        interfaces: &HashSet<WitInterface>,
        resolved: &BTreeMap<String, HashMap<String, String>>,
    ) -> HashSet<WitInterface> {
        interfaces
            .iter()
            .map(|interface| {
                let mut stamped = interface.clone();
                if let Some(config) = resolved.get(self.binding_name(interface)) {
                    stamped.config = config.clone();
                }
                stamped
            })
            .collect()
    }

    /// Merge the operator's declaration for `binding` with what a workload
    /// wrote, refusing under [`WorkloadConfigPolicy::Deny`] what the host owns.
    ///
    /// Precedence is host layer, then workload config — last wins — compared by
    /// canonical key, so a workload's `subject_allow` replaces the operator's
    /// `subject-allow` rather than sitting beside it. Under `Deny` no key the
    /// host owns survives the check, so that ordering only ever applies to keys
    /// the workload is entitled to write.
    ///
    /// # Errors
    ///
    /// Under `Deny`: a `binding` name the operator did not declare, a workload
    /// config that sets a [`KeyOwnership::Host`] key, one that widens a
    /// [`KeyOwnership::HostCeiling`] key, or one that sets a key a closed
    /// schema does not know. Under [`WorkloadConfigPolicy::Warn`] the same
    /// findings are logged and resolution proceeds; under
    /// [`WorkloadConfigPolicy::Allow`] they are neither.
    pub fn resolve(
        &self,
        binding: &str,
        workload: &HashMap<String, String>,
        schema: &BindingSchema,
        narrows: NarrowsFn<'_>,
    ) -> anyhow::Result<HashMap<String, String>> {
        if self.workload_config.reports() {
            let host_layer = self.host_layer(binding);
            let findings = self.findings(binding, workload, &host_layer, schema, narrows);
            if !findings.is_empty() {
                if self.workload_config.enforces() {
                    anyhow::bail!("{}", findings.join("; "));
                }
                for finding in findings {
                    warn!(
                        plugin_id = %self.plugin_id,
                        binding = describe_binding(binding),
                        "`workloadConfig: deny` would refuse this workload — {finding}"
                    );
                }
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
        self.apply_default_bundles(binding, &mut resolved);
        Ok(resolved)
    }

    /// Everything `Deny` would refuse about `workload`, in a stable order.
    ///
    /// One function for both modes, so `Warn` reports exactly what `Deny`
    /// enforces however the refusal set grows.
    fn findings(
        &self,
        binding: &str,
        workload: &HashMap<String, String>,
        host_layer: &HashMap<String, String>,
        schema: &BindingSchema,
        narrows: NarrowsFn<'_>,
    ) -> Vec<String> {
        let mut findings = Vec::new();

        // Falling back to the base config for an undeclared name would start
        // the workload against the right backend with none of the grants the
        // operator meant it to have, and every call would be refused one at a
        // time with nothing pointing at the missing declaration.
        //
        // Only once the operator has declared *something*, though: a plugin
        // with no declared bindings is not an operator saying "these are the
        // ones I serve", it is an operator who has not spoken. Refusing every
        // label there would break named routing on every host that never wrote
        // a `host.plugins` entry, which is what makes `Deny` safe as the
        // default.
        if binding != UNNAMED_BINDING
            && !self.bindings.is_empty()
            && !self.bindings.contains_key(binding)
        {
            findings.push(format!(
                "this plugin declares bindings{} and `{binding}` is not among them; add it under \
                 `host.plugins` entry `{}`'s `bindings`",
                self.describe_available(),
                self.plugin_id,
            ));
        }

        let unknown = schema.unknown_keys(workload);
        if !unknown.is_empty() {
            findings.push(format!(
                "it sets {}, which this plugin does not read; a key it does not recognize is \
                 silently ignored, so a typo in a grant or a credential would leave the binding \
                 configured as if nothing had been written",
                describe_unknown(&unknown),
            ));
        }

        // Sorted: `workload` is a `HashMap`, and the same manifest has to be
        // reported with the same key named every time.
        let mut keys: Vec<&String> = workload.keys().collect();
        keys.sort();

        let mut refused: Vec<String> = Vec::new();
        for key in &keys {
            if self.ownership_of(key, schema) == KeyOwnership::Host {
                refused.push(format!("`{key}`"));
            }
        }
        if !refused.is_empty() {
            findings.push(format!(
                "it sets {}, which belong to the operator; a manifest that set them could point \
                 itself at another backend or widen its own grant. Ask the operator to declare \
                 them under `host.plugins`",
                refused.join(", "),
            ));
        }

        for key in keys {
            if self.ownership_of(key, schema) != KeyOwnership::HostCeiling {
                continue;
            }
            let Some(workload_value) = workload.get(key) else {
                continue;
            };
            match lookup(host_layer, key) {
                // No ceiling was declared, so nothing contains the request. An
                // ungranted allowlist has to resolve to empty, not to whatever
                // the manifest wrote.
                None => findings.push(format!(
                    "it sets `{key}`, which this host declares no ceiling for; a grant the \
                     operator never declared cannot be narrowed into"
                )),
                Some(ceiling) if !narrows(&canonical_key(key), ceiling, workload_value) => {
                    findings.push(format!(
                        "`{key}: {workload_value}` is not within the grant this host declared \
                         (`{ceiling}`){}. A workload may narrow a grant, never widen one",
                        // The predicate is the only thing that knows what
                        // containment means, so it is also what localizes the
                        // failure: re-ask it per element to name the one that
                        // does not fit.
                        match first_widening_element(
                            &canonical_key(key),
                            ceiling,
                            workload_value,
                            narrows
                        ) {
                            Some(element) => format!(" — `{element}` is outside it"),
                            None => String::new(),
                        }
                    ));
                }
                Some(_) => {}
            }
        }

        findings
    }

    /// Apply every default bundle whose anchor nobody set.
    fn apply_default_bundles(&self, binding: &str, resolved: &mut HashMap<String, String>) {
        for bundle in &self.default_bundles {
            if lookup(resolved, &bundle.anchor).is_some() {
                info!(
                    plugin_id = %self.plugin_id,
                    binding = describe_binding(binding),
                    anchor = %bundle.anchor,
                    skipped = %bundle
                        .entries
                        .iter()
                        .map(|(k, _)| k.as_str())
                        .filter(|k| *k != bundle.anchor)
                        .collect::<Vec<_>>()
                        .join(", "),
                    "`{}` was set, so its default bundle was not applied",
                    bundle.anchor
                );
                continue;
            }
            for (key, value) in &bundle.entries {
                if lookup(resolved, key).is_none() {
                    resolved.insert(key.clone(), value.clone());
                }
            }
        }
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

    /// Refuse a declaration that could never be read.
    ///
    /// Today that is one case: a binding named after its own plugin. A label
    /// equal to the plugin's id routes to the unnamed binding, so
    /// `bindings.<plugin-id>` is silently dead — the operator's keys sit there
    /// and nothing consults them. Symmetric with the empty-binding-name
    /// refusal, and worth having however the plugin-id routing axis is
    /// eventually spelled: a declaration that can never be read is wrong under
    /// any routing rule.
    ///
    /// # Errors
    ///
    /// Names the binding and where its keys belong instead.
    pub fn validate_declaration(&self) -> anyhow::Result<()> {
        if self.bindings.contains_key(&self.plugin_id) {
            anyhow::bail!(
                "`host.plugins` entry `{0}` declares a binding named `{0}`, which is \
                 unreachable: a label equal to the plugin's id routes to the unnamed binding, so \
                 this declaration would never be used. Move its keys to the entry's own \
                 `config`, or rename the binding",
                self.plugin_id
            )
        }
        Ok(())
    }

    /// Refuse any key the operator wrote that the plugin does not read.
    ///
    /// The operator's half of [`BindingSchema::reject_unknown_keys`], checked
    /// once at host startup rather than per workload. A no-op on an open schema.
    ///
    /// # Errors
    ///
    /// Names the entry or binding the unknown key was written on.
    pub fn reject_unknown_keys(&self, schema: &BindingSchema) -> anyhow::Result<()> {
        schema.reject_unknown_keys(
            &self.base,
            &format!("`host.plugins` entry `{}`", self.plugin_id),
        )?;
        for (name, config) in &self.bindings {
            schema.reject_unknown_keys(
                config,
                &format!("`host.plugins` entry `{}` binding `{name}`", self.plugin_id),
            )?;
        }
        Ok(())
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

/// Read `key` from `config` in any spelling.
fn lookup<'a>(config: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    let key = canonical_key(key);
    config
        .iter()
        .find(|(k, _)| canonical_key(k) == key)
        .map(|(_, v)| v.as_str())
}

/// The first comma-separated element of `workload` the predicate refuses.
///
/// A courtesy, not a check: the whole value has already been refused. Naming
/// the offending element rather than the whole list is what turns a policy
/// error into an edit. `None` when the value is not a list, or when no single
/// element is individually refused — the refusal then stands on its own.
fn first_widening_element(
    key: &str,
    host: &str,
    workload: &str,
    narrows: NarrowsFn<'_>,
) -> Option<String> {
    let elements: Vec<&str> = workload
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .collect();
    if elements.len() < 2 {
        return None;
    }
    elements
        .into_iter()
        .find(|element| !narrows(key, host, element))
        .map(str::to_string)
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

    /// The declaration for `plugin_id`, or an empty one.
    ///
    /// The empty one is shared and carries no plugin id, which is fine for
    /// resolving — nothing is declared, so nothing names it. Use
    /// [`PluginBindings::entry`] to build on it.
    #[must_use]
    pub fn for_plugin(&self, plugin_id: &str) -> &PluginBindingSet {
        self.plugins.get(plugin_id).unwrap_or(&UNDECLARED)
    }

    /// An owned declaration for `plugin_id` to add to: whatever the operator
    /// declared, or a fresh one carrying the right id.
    ///
    /// What a front end uses to layer its own flag-derived defaults on top of
    /// the config file, since [`PluginBindings::for_plugin`]'s fallback has no
    /// id and would re-insert under the wrong key.
    #[must_use]
    pub fn entry(&self, plugin_id: &str) -> PluginBindingSet {
        self.plugins
            .get(plugin_id)
            .cloned()
            .unwrap_or_else(|| PluginBindingSet::new(plugin_id))
    }

    /// Two conditions, and only one of them should stop a host from starting —
    /// see [`crate::plugin::KNOWN_PLUGIN_IDS`]. An id nobody has heard of is a
    /// typo, possibly shadowing the plugin the operator meant to constrain. An
    /// id this build did not compile in is a chart/binary skew: nothing can
    /// bind the plugin either, so the declaration is inert in both directions
    /// and refusing to boot turns a skew into an outage.
    ///
    /// # Errors
    ///
    /// Names every declared id absent from [`crate::plugin::KNOWN_PLUGIN_IDS`],
    /// with the nearest registered id suggested where there is one.
    pub fn validate_against(&self, registered: &[&str]) -> anyhow::Result<()> {
        let mut unknown: Vec<String> = Vec::new();
        for id in self.plugin_ids() {
            if registered.contains(&id) {
                continue;
            }
            if crate::plugin::KNOWN_PLUGIN_IDS.contains(&id) {
                warn!(
                    plugin_id = %id,
                    "`host.plugins` entry names a plugin this host was not built with; its \
                     declaration is ignored"
                );
                continue;
            }
            let nearest = registered
                .iter()
                .chain(crate::plugin::KNOWN_PLUGIN_IDS.iter())
                .find(|known| within_one_edit(id, known));
            unknown.push(match nearest {
                Some(near) => format!("`{id}` (did you mean `{near}`?)"),
                None => format!("`{id}`"),
            });
        }
        if unknown.is_empty() {
            return Ok(());
        }
        let mut known: Vec<&str> = registered.to_vec();
        known.sort_unstable();
        anyhow::bail!(
            "`host.plugins` declares {}, which names no known plugin. Registered plugins: {}",
            unknown.join(", "),
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

    /// A closed schema in the shape the NATS plugin will declare: connection and
    /// grant keys owned by the host, the rest named so a typo is caught.
    fn nats_schema() -> BindingSchema {
        BindingSchema::with_host_owned_keys([
            "servers",
            "creds",
            "inbox-prefix",
            "jetstream-domain",
        ])
        .and_host_ceiling_keys(["subject-allow", "stream-allow"])
        .and_workload_owned_keys(["ack-mode", "core-subscriptions"])
    }

    /// A containment predicate over comma-separated prefixes, standing in for a
    /// plugin's own: `orders.received` is inside `orders.>`.
    fn prefix_narrows() -> NarrowsFn<'static> {
        fn split(s: &str) -> Vec<&str> {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect()
        }
        &|_key: &str, host: &str, workload: &str| {
            let ceiling = split(host);
            split(workload).into_iter().all(|w| {
                ceiling
                    .iter()
                    .any(|c| *c == w || c.strip_suffix('>').is_some_and(|p| w.starts_with(p)))
            })
        }
    }

    #[test]
    fn undeclared_plugin_resolves_passthrough_under_the_deny_default() {
        // What makes `Deny` safe as the default: nothing declared means no keys
        // owned, no names claimed, and no schema to check, so a host whose
        // operator wrote no `host.plugins` behaves exactly as before.
        let bindings = PluginBindings::new();
        let set = bindings.for_plugin("wasmcloud-nats");
        assert_eq!(set.workload_config(), WorkloadConfigPolicy::Deny);

        let interfaces: HashSet<_> = [
            iface(None, &[("servers", "nats://guest:4222")]),
            iface(Some("some-label"), &[("bucket", "cache")]),
        ]
        .into();

        let resolved = set
            .resolve_by_name(&interfaces, &BindingSchema::empty(), never_narrows())
            .unwrap();
        assert_eq!(resolved[""]["servers"], "nats://guest:4222");
        assert_eq!(resolved["some-label"]["bucket"], "cache");
        assert_eq!(set.apply_resolved(&interfaces, &resolved), interfaces);
    }

    #[test]
    fn deny_claims_no_names_until_the_operator_declares_one() {
        // An operator who declared only host-wide config has not said "these are
        // the bindings I serve", so a label still routes; one who declared a
        // binding has, so an unlisted label is refused.
        let quiet = PluginBindingSet::new("kv").with_base(map(&[("url", "redis://host:6379")]));
        let resolved = quiet
            .resolve(
                "sessions",
                &HashMap::new(),
                &BindingSchema::empty(),
                never_narrows(),
            )
            .unwrap();
        assert_eq!(resolved["url"], "redis://host:6379");

        let spoken = quiet.clone().with_binding("cache", map(&[("bucket", "c")]));
        assert!(
            spoken
                .resolve(
                    "sessions",
                    &HashMap::new(),
                    &BindingSchema::empty(),
                    never_narrows()
                )
                .is_err()
        );
    }

    #[test]
    fn a_closed_schema_refuses_an_unknown_key_and_suggests_the_near_miss() {
        // The check that keeps the lists honest: a key the plugin does not read
        // is a typo or a stale doc, and silently ignoring it leaves a grant
        // configured as if nothing had been written.
        let schema = nats_schema();
        let set = PluginBindingSet::new("wasmcloud-nats").with_binding("orders", HashMap::new());

        let err = set
            .resolve(
                "orders",
                &map(&[("subject-alow", "orders.>")]),
                &schema,
                never_narrows(),
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("`subject-alow`"), "got: {err}");
        assert!(err.contains("did you mean `subject-allow`"), "got: {err}");

        // A key with no near miss is still refused, just without a suggestion.
        let err = set
            .resolve(
                "orders",
                &map(&[("totally-made-up", "1")]),
                &schema,
                never_narrows(),
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("`totally-made-up`"), "got: {err}");
        assert!(!err.contains("did you mean"), "got: {err}");
    }

    #[test]
    fn an_open_schema_still_passes_unknown_keys_through() {
        // A plugin that names only its host-owned keys has not claimed to have
        // listed everything it reads, so nothing is refused as unknown.
        let open = BindingSchema::with_host_owned_keys(["servers"]);
        assert!(!open.is_closed());

        let set = PluginBindingSet::new("p").with_workload_config(WorkloadConfigPolicy::Allow);
        let resolved = set
            .resolve(
                "",
                &map(&[("anything-at-all", "1")]),
                &open,
                never_narrows(),
            )
            .unwrap();
        assert_eq!(resolved["anything-at-all"], "1");
    }

    #[test]
    fn an_operators_own_typo_is_refused_at_startup() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_base(map(&[("server", "nats://host:4222")]))
            .with_binding("orders", map(&[("stream-alow", "ORDERS")]));

        let err = set
            .reject_unknown_keys(&nats_schema())
            .unwrap_err()
            .to_string();
        assert!(err.contains("`server`"), "got: {err}");
        assert!(err.contains("did you mean `servers`"), "got: {err}");

        set.reject_unknown_keys(&BindingSchema::empty()).unwrap();
    }

    #[test]
    fn allow_layers_workload_over_the_operator() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_base(map(&[
                ("servers", "nats://host:4222"),
                ("creds", "/h.creds"),
            ]))
            .with_binding("orders", map(&[("subject-allow", "orders.>")]))
            .with_workload_config(WorkloadConfigPolicy::Allow);

        let resolved = set
            .resolve(
                "orders",
                &map(&[("servers", "nats://guest:4222"), ("ack-mode", "manual")]),
                &nats_schema(),
                never_narrows(),
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
        let set = PluginBindingSet::new("p")
            .with_base(map(&[("ack_mode", "auto")]))
            .with_workload_config(WorkloadConfigPolicy::Allow);
        let resolved = set
            .resolve(
                "",
                &map(&[("ack-mode", "manual")]),
                &BindingSchema::empty(),
                never_narrows(),
            )
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved.get("ack-mode").unwrap(), "manual");
    }

    #[test]
    fn deny_refuses_a_host_owned_key_in_either_spelling() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_binding("orders", map(&[("subject-allow", "orders.>")]))
            .with_workload_config(WorkloadConfigPolicy::Deny);

        // `servers` is the host's outright: an address has no "less".
        for spelling in ["servers", "SERVERS", "Servers"] {
            let err = set
                .resolve(
                    "orders",
                    &map(&[(spelling, "nats://elsewhere:4222")]),
                    &nats_schema(),
                    never_narrows(),
                )
                .unwrap_err()
                .to_string();
            assert!(err.contains(spelling), "got: {err}");
            assert!(err.contains("belong to the operator"), "got: {err}");
        }

        // A grant is a ceiling, so the refusal is about widening rather than
        // about touching the key — and `_` is the same key as `-`.
        for spelling in ["subject-allow", "subject_allow"] {
            let err = set
                .resolve(
                    "orders",
                    &map(&[(spelling, ">")]),
                    &nats_schema(),
                    never_narrows(),
                )
                .unwrap_err()
                .to_string();
            assert!(err.contains(spelling), "got: {err}");
            assert!(err.contains("never widen one"), "got: {err}");
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
                never_narrows(),
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
            .resolve("shipping", &HashMap::new(), &nats_schema(), never_narrows())
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
        let resolved = set
            .resolve("", &HashMap::new(), &nats_schema(), never_narrows())
            .unwrap();
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
        // The whole point of `binding_schema`: a grant nobody declared resolves
        // to empty rather than to whatever the manifest wrote.
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_workload_config(WorkloadConfigPolicy::Deny);
        let err = set
            .resolve(
                "",
                &map(&[("subject-allow", ">")]),
                &nats_schema(),
                prefix_narrows(),
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("declares no ceiling"), "got: {err}");
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
        let resolved = set
            .resolve_by_name(&interfaces, &nats_schema(), never_narrows())
            .unwrap();
        assert_eq!(resolved[UNNAMED_BINDING]["servers"], "nats://host:4222");
    }

    #[test]
    fn resolve_by_name_resolves_each_label_against_its_own_binding() {
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
        let by_name = set
            .resolve_by_name(&interfaces, &nats_schema(), never_narrows())
            .unwrap();
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
        // Two host-owned keys, so they land in one finding and their order in
        // it is the thing under test.
        let workload = map(&[("creds", "/x.creds"), ("servers", "nats://x:4222")]);
        let first = set
            .resolve("", &workload, &nats_schema(), never_narrows())
            .unwrap_err()
            .to_string();
        for _ in 0..16 {
            assert_eq!(
                set.resolve("", &workload, &nats_schema(), never_narrows())
                    .unwrap_err()
                    .to_string(),
                first
            );
        }
        // Sorted, so `creds` leads.
        assert!(first.contains("`creds`, `servers`"), "got: {first}");
    }

    #[test]
    fn a_workload_may_narrow_a_ceiling_and_not_widen_it() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_binding("orders", map(&[("subject-allow", "orders.>")]))
            .with_workload_config(WorkloadConfigPolicy::Deny);

        let resolved = set
            .resolve(
                "orders",
                &map(&[("subject-allow", "orders.received")]),
                &nats_schema(),
                prefix_narrows(),
            )
            .expect("asking for less than the ceiling is the point of a ceiling");
        assert_eq!(
            resolved["subject-allow"], "orders.received",
            "the resolved value is the manifest's, never a computed intersection"
        );

        // A workload that sets nothing takes the whole ceiling: narrowing is
        // opt-in, so the default posture stays the maximum the operator gave.
        let untouched = set
            .resolve("orders", &HashMap::new(), &nats_schema(), prefix_narrows())
            .unwrap();
        assert_eq!(untouched["subject-allow"], "orders.>");

        let err = set
            .resolve(
                "orders",
                &map(&[("subject-allow", "orders.received,billing.>")]),
                &nats_schema(),
                prefix_narrows(),
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("`billing.>` is outside it"), "got: {err}");
    }

    #[test]
    fn host_owned_keys_upgrades_a_ceiling_to_the_hosts_alone() {
        // Naming a key explicitly reads as "mine", and hands an operator a
        // per-key opt-out of narrowing without a change to the plugin.
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_binding("orders", map(&[("subject-allow", "orders.>")]))
            .with_host_owned_keys(["subject-allow"])
            .with_workload_config(WorkloadConfigPolicy::Deny);

        assert_eq!(
            set.ownership_of("subject-allow", &nats_schema()),
            KeyOwnership::Host
        );
        let err = set
            .resolve(
                "orders",
                &map(&[("subject-allow", "orders.received")]),
                &nats_schema(),
                prefix_narrows(),
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("belong to the operator"), "got: {err}");
    }

    #[test]
    fn warn_reports_everything_deny_would_refuse_and_changes_nothing() {
        let declaration = |policy| {
            PluginBindingSet::new("wasmcloud-nats")
                .with_base(map(&[("servers", "nats://host:4222")]))
                .with_binding("orders", map(&[("subject-allow", "orders.>")]))
                .with_workload_config(policy)
        };
        let workload = map(&[("servers", "nats://elsewhere:4222")]);

        declaration(WorkloadConfigPolicy::Deny)
            .resolve("orders", &workload, &nats_schema(), prefix_narrows())
            .expect_err("deny refuses");

        // `warn` must not change which value wins, or flipping it to `deny`
        // would not be a no-op when the log is quiet.
        let warned = declaration(WorkloadConfigPolicy::Warn)
            .resolve("orders", &workload, &nats_schema(), prefix_narrows())
            .expect("warn refuses nothing");
        let allowed = declaration(WorkloadConfigPolicy::Allow)
            .resolve("orders", &workload, &nats_schema(), prefix_narrows())
            .expect("allow refuses nothing");
        assert_eq!(warned, allowed);
        assert_eq!(warned["servers"], "nats://elsewhere:4222");
    }

    #[test]
    fn a_default_bundle_travels_together_and_is_skipped_as_a_whole() {
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_workload_config(WorkloadConfigPolicy::Allow)
            .with_default_bundle(
                "servers",
                [
                    ("servers", "nats://data:4222"),
                    ("tls-ca", "/certs/ca.crt"),
                    ("tls-cert", "/certs/tls.crt"),
                ],
            );

        let inherited = set
            .resolve(
                "",
                &HashMap::new(),
                &BindingSchema::empty(),
                never_narrows(),
            )
            .unwrap();
        assert_eq!(inherited["servers"], "nats://data:4222");
        assert_eq!(inherited["tls-ca"], "/certs/ca.crt");

        // Anchored: a binding pointed somewhere else takes none of it, because
        // certs are only valid for the address they were issued for.
        let elsewhere = set
            .resolve(
                "",
                &map(&[("servers", "nats://elsewhere:4222")]),
                &BindingSchema::empty(),
                never_narrows(),
            )
            .unwrap();
        assert_eq!(elsewhere["servers"], "nats://elsewhere:4222");
        assert!(!elsewhere.contains_key("tls-ca"), "{elsewhere:?}");
        assert!(!elsewhere.contains_key("tls-cert"), "{elsewhere:?}");
    }

    #[test]
    fn a_binding_named_after_its_own_plugin_is_refused() {
        let err = PluginBindingSet::new("wasmcloud-nats")
            .with_binding("wasmcloud-nats", map(&[("servers", "nats://x:4222")]))
            .validate_declaration()
            .unwrap_err()
            .to_string();
        assert!(err.contains("unreachable"), "got: {err}");

        PluginBindingSet::new("wasmcloud-nats")
            .with_binding("orders", HashMap::new())
            .validate_declaration()
            .unwrap();
    }

    #[test]
    fn a_plugin_this_build_lacks_warns_rather_than_refusing() {
        // A chart that renders an entry unconditionally must not turn a
        // feature-flag skew into a host that will not boot.
        PluginBindings::new()
            .with_plugin(PluginBindingSet::new("wasmcloud-nats"))
            .validate_against(&["wasi-keyvalue"])
            .expect("a known plugin this build lacks is inert, not fatal");

        let err = PluginBindings::new()
            .with_plugin(PluginBindingSet::new("wasmcloud-nat"))
            .validate_against(&["wasi-keyvalue"])
            .unwrap_err()
            .to_string();
        assert!(err.contains("`wasmcloud-nat`"), "got: {err}");
        assert!(err.contains("did you mean `wasmcloud-nats`"), "got: {err}");
    }

    #[test]
    fn entries_of_one_label_fold_before_the_host_layer_lands() {
        // The sequencing that matters: an operator-set key a workload overrides
        // on one entry and not another must not come back differing between
        // them and be refused as a conflict the workload never wrote.
        let set = PluginBindingSet::new("wasmcloud-nats")
            .with_base(map(&[("servers", "nats://host:4222")]))
            .with_workload_config(WorkloadConfigPolicy::Allow);

        let interfaces: HashSet<_> = [
            iface(None, &[("servers", "nats://guest:4222")]),
            iface(None, &[("core-subscriptions", "orders.new")]),
        ]
        .into();

        let resolved = set
            .resolve_by_name(&interfaces, &nats_schema(), never_narrows())
            .unwrap();
        assert_eq!(resolved[""]["servers"], "nats://guest:4222");
        assert_eq!(resolved[""]["core-subscriptions"], "orders.new");
    }

    #[test]
    fn entries_of_one_label_that_disagree_are_refused_by_key_name() {
        let set = PluginBindingSet::new("wasmcloud-nats");
        let interfaces: HashSet<_> = [
            iface(None, &[("servers", "nats://a:4222")]),
            iface(None, &[("SERVERS", "nats://b:4222")]),
        ]
        .into();
        let err = set
            .resolve_by_name(&interfaces, &BindingSchema::empty(), never_narrows())
            .unwrap_err()
            .to_string();
        assert!(err.contains("conflicting values for `servers`"), "{err}");
        assert!(!err.contains("nats://"), "message leaks values: {err}");
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
