//! Plugin-backed `secrets:` entries: config keys of one plugin's bindings whose
//! values another plugin serves through `wasmcloud:host/secret-source`.
//!
//! The key names are part of the [`PluginBindingSet`] from host start, so every
//! key-level check sees them ([`validate_plugin_secrets`]). The values are
//! resolved each time a workload binds, before the binding policy runs
//! ([`resolve_plugin_secrets`]), after which a secret key is an ordinary
//! operator config key.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use anyhow::Context as _;

use super::HostPlugin;
use super::bindings::{BindingSchema, KeyOwnership, PluginBindingSet};
use crate::wit::WitInterface;

/// A string-backed name, typed so one kind of name can't be passed as another.
macro_rules! name_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
    };
}

name_type!(
    /// The `id` of a `host.plugins` entry.
    PluginId
);
name_type!(
    /// The name of a binding under a `host.plugins` entry.
    BindingName
);
name_type!(
    /// The name of an entry in the top-level `secrets:` catalog.
    CatalogName
);
name_type!(
    /// A key in a secret source's backend, as written under a catalog entry's
    /// `keys`.
    SecretKey
);

/// The workloads a plugin-backed `secrets:` entry is granted to. Every entry
/// states one, so none reaches a workload the operator did not name.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(try_from = "RawWorkloadGrant", into = "RawWorkloadGrant")]
pub enum WorkloadGrant {
    /// Every workload on the host: `workloads: all`.
    All,
    /// Workloads matching one of these, never empty.
    Only(Vec<WorkloadSelector>),
}

/// Workloads in `namespace`, or only the one called `name` there.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkloadSelector {
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl WorkloadSelector {
    /// Every workload in `namespace`.
    pub fn namespace(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: None,
        }
    }

    /// The workload called `name` in `namespace`.
    pub fn workload(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: Some(name.into()),
        }
    }

    fn admits(&self, workload: &RequestingWorkload) -> bool {
        self.namespace == workload.namespace
            && self.name.as_ref().is_none_or(|name| *name == workload.name)
    }
}

impl WorkloadGrant {
    /// Whether `workload` may bind a binding that reads the entry.
    pub fn admits(&self, workload: &RequestingWorkload) -> bool {
        match self {
            Self::All => true,
            Self::Only(selectors) => selectors.iter().any(|s| s.admits(workload)),
        }
    }
}

impl std::fmt::Display for WorkloadGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => f.write_str("every workload"),
            Self::Only(selectors) => {
                let names: Vec<String> = selectors
                    .iter()
                    .map(|s| match &s.name {
                        Some(name) => format!("`{}/{name}`", s.namespace),
                        None => format!("namespace `{}`", s.namespace),
                    })
                    .collect();
                f.write_str(&names.join(", "))
            }
        }
    }
}

/// The YAML spelling of [`WorkloadGrant`]: the keyword `all`, or a list of
/// selectors.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum RawWorkloadGrant {
    All(AllWorkloads),
    Only(Vec<WorkloadSelector>),
}

#[derive(serde::Serialize, serde::Deserialize)]
enum AllWorkloads {
    #[serde(rename = "all")]
    All,
}

impl TryFrom<RawWorkloadGrant> for WorkloadGrant {
    type Error = &'static str;

    fn try_from(raw: RawWorkloadGrant) -> Result<Self, Self::Error> {
        match raw {
            RawWorkloadGrant::All(AllWorkloads::All) => Ok(Self::All),
            RawWorkloadGrant::Only(selectors) if selectors.is_empty() => {
                Err("`workloads` grants no workload; name at least one namespace, or write `all`")
            }
            RawWorkloadGrant::Only(selectors) => Ok(Self::Only(selectors)),
        }
    }
}

impl From<WorkloadGrant> for RawWorkloadGrant {
    fn from(grant: WorkloadGrant) -> Self {
        match grant {
            WorkloadGrant::All => Self::All(AllWorkloads::All),
            WorkloadGrant::Only(selectors) => Self::Only(selectors),
        }
    }
}

/// What the host asks a [`HostSecretSource`] for, and on whose behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRequest {
    /// The binding the values are delivered to.
    pub consumer: SecretConsumer,
    /// The workload whose bind needs them.
    pub workload: RequestingWorkload,
    /// The keys to resolve, without duplicates.
    pub keys: Vec<SecretKey>,
}

/// The binding a [`SecretRequest`]'s values are delivered to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretConsumer {
    /// The plugin the values are for.
    pub plugin: PluginId,
    /// The binding of `plugin` the source is declared on, or `None` for the
    /// entry itself.
    pub binding: Option<BindingName>,
}

/// The workload whose bind a [`SecretRequest`] is made for, from the host's own
/// records of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestingWorkload {
    pub id: String,
    pub name: String,
    pub namespace: String,
}

/// A secret's value: text, or bytes for binary material. Redacted in `Debug`,
/// with no `Display`, and zeroed on drop, so reading it is always an explicit
/// [`Self::expose`].
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(SecretBytes);

#[derive(Clone, PartialEq, Eq)]
enum SecretBytes {
    Text(String),
    Bytes(Vec<u8>),
}

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(SecretBytes::Text(value.into()))
    }

    pub fn from_bytes(value: impl Into<Vec<u8>>) -> Self {
        Self(SecretBytes::Bytes(value.into()))
    }

    /// The value as text, or `None` for bytes that are not UTF-8. Never log
    /// it.
    pub fn expose(&self) -> Option<&str> {
        match &self.0 {
            SecretBytes::Text(text) => Some(text),
            SecretBytes::Bytes(bytes) => std::str::from_utf8(bytes).ok(),
        }
    }

    /// The value as text, for handing to a consumer whose config takes text,
    /// or `None` for bytes that are not UTF-8. Never log it.
    pub fn into_text(mut self) -> Option<String> {
        match std::mem::replace(&mut self.0, SecretBytes::Text(String::new())) {
            SecretBytes::Text(text) => Some(text),
            SecretBytes::Bytes(bytes) => match String::from_utf8(bytes) {
                Ok(text) => Some(text),
                Err(e) => {
                    zeroize::Zeroize::zeroize(&mut e.into_bytes());
                    None
                }
            },
        }
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        match &mut self.0 {
            SecretBytes::Text(text) => zeroize::Zeroize::zeroize(text),
            SecretBytes::Bytes(bytes) => zeroize::Zeroize::zeroize(bytes),
        }
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

/// Why a [`HostSecretSource`] did not resolve a request: the
/// `wasmcloud:host/secret-source` `source-error` cases, or the call itself
/// failing. Each message is the source's own and never carries a value.
#[derive(Debug)]
pub enum SecretSourceError {
    /// These requested keys do not exist.
    NotFound(Vec<SecretKey>),
    /// The source refuses these keys to this consumer.
    AccessDenied(Option<String>),
    /// The backend could not be reached; a later bind may succeed.
    Unavailable(Option<String>),
    /// Any other failure the source reported.
    Other(Option<String>),
    /// The source answered `ok`, but not with exactly the requested keys,
    /// each once.
    InvalidAnswer(String),
    /// The source was not called, or the call did not return: the plugin is
    /// not running, trapped, or timed out.
    Call(anyhow::Error),
}

impl std::fmt::Display for SecretSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (case, detail) = match self {
            Self::AccessDenied(detail) => ("access denied", detail),
            Self::Unavailable(detail) => ("unavailable", detail),
            Self::Other(detail) => ("failed", detail),
            Self::NotFound(keys) => {
                let keys: Vec<String> = keys.iter().map(|k| format!("`{k}`")).collect();
                return write!(f, "no key {}", keys.join(", "));
            }
            Self::Call(e) => return write!(f, "the call failed: {e:#}"),
            Self::InvalidAnswer(detail) => return write!(f, "invalid answer: {detail}"),
        };
        match detail {
            Some(detail) => write!(f, "{case}: {detail}"),
            None => f.write_str(case),
        }
    }
}

impl std::error::Error for SecretSourceError {}

/// A plugin that serves secrets to the host for other plugins' bindings.
#[async_trait::async_trait]
pub trait HostSecretSource: Send + Sync {
    /// The value of each key in `request.keys`, keyed by it.
    async fn resolve(
        &self,
        request: SecretRequest,
    ) -> Result<BTreeMap<SecretKey, SecretValue>, SecretSourceError>;
}

/// A source as declared on one layer: the binding (`None` for the entry) and
/// the `secrets:` entry name.
type LayerSource<'a> = (Option<&'a str>, &'a str);

/// Check `plugin_id`'s plugin secrets at host start: each must name another
/// registered plugin that serves `wasmcloud:host/secret-source`, and none may
/// fill a ceiling key, which is checked against its declared value.
pub(crate) fn validate_plugin_secrets(
    plugin_id: &str,
    declared: &PluginBindingSet,
    schema: &BindingSchema,
    plugins: &HashMap<&'static str, Arc<dyn HostPlugin>>,
) -> anyhow::Result<()> {
    for (binding, key, secret) in declared.plugin_secrets() {
        let place = match binding {
            Some(name) => format!("`host.plugins` entry `{plugin_id}` binding `{name}`"),
            None => format!("`host.plugins` entry `{plugin_id}`"),
        };
        let source = &secret.source;
        anyhow::ensure!(
            secret.plugin.as_str() != plugin_id,
            "{place}: secret `{source}` names this plugin, which cannot supply its own config"
        );
        anyhow::ensure!(
            plugins
                .get(secret.plugin.as_str())
                .is_some_and(|p| p.secret_source().is_some()),
            "{place}: secret `{source}` names plugin `{}`, which is not registered or does not \
             export wasmcloud:host/secret-source",
            secret.plugin
        );
        anyhow::ensure!(
            declared.ownership_of(key, schema) != KeyOwnership::HostCeiling,
            "{place}: `{key}` is a ceiling, checked against the value declared at host start, so \
             secret `{source}` cannot supply it"
        );
    }
    Ok(())
}

/// What [`resolve_plugin_secrets`] filled in for one bind.
#[derive(Debug)]
pub(crate) struct FilledSecrets {
    /// The declaration with this bind's plugin secrets written in.
    pub set: PluginBindingSet,
    /// The named bindings whose layers changed, or `None` when nothing was
    /// filled: what the plugin checks again.
    pub refilled: Option<BTreeSet<String>>,
    /// Scrubs the filled values out of an error.
    pub redactor: SecretRedactor,
}

/// The values a bind filled in, kept only to keep them out of its errors.
#[derive(Debug, Default)]
pub(crate) struct SecretRedactor(Vec<SecretValue>);

impl SecretRedactor {
    /// `e` with every value filled in replaced by `<redacted>`, since a
    /// plugin's parser may quote the value it refuses.
    pub fn redact(&self, e: anyhow::Error) -> anyhow::Error {
        let mut secrets: Vec<&str> = self
            .0
            .iter()
            .filter_map(SecretValue::expose)
            .flat_map(|v| [v, v.trim()])
            .filter(|v| !v.is_empty())
            .collect();
        let msg = format!("{e:#}");
        if !secrets.iter().any(|v| msg.contains(v)) {
            return e;
        }
        // Longest first, so a value containing another is replaced whole.
        secrets.sort_by_key(|v| std::cmp::Reverse(v.len()));
        let msg = secrets
            .into_iter()
            .fold(msg, |msg, v| msg.replace(v, "<redacted>"));
        anyhow::anyhow!(msg)
    }
}

/// `declared` with the plugin secrets of every binding in `interfaces` filled
/// in. Each source declared on a layer is resolved with one call, keys
/// deduplicated, however many keys and entries read it; the calls run
/// concurrently. An entry-level secret every binding in use sets for itself is
/// not resolved.
///
/// # Errors
///
/// A source that is unavailable, refuses, or leaves a key out. Errors name the
/// entry, source and key, never a value; a source's own refusal is a
/// [`SecretSourceError`] in the chain.
pub(crate) async fn resolve_plugin_secrets(
    plugins: &HashMap<&'static str, Arc<dyn HostPlugin>>,
    workload: &RequestingWorkload,
    declared: PluginBindingSet,
    interfaces: &HashSet<WitInterface>,
    schema: &BindingSchema,
) -> anyhow::Result<FilledSecrets> {
    if !declared.has_plugin_secrets() {
        return Ok(FilledSecrets {
            set: declared,
            refilled: None,
            redactor: SecretRedactor::default(),
        });
    }
    let plugin_id = declared.plugin_id().to_string();
    let in_use: BTreeSet<&str> = interfaces
        .iter()
        .map(|i| declared.binding_name(i))
        .collect();
    // The entry-level keys some binding in use still reads.
    let base_read: BTreeSet<String> = in_use
        .iter()
        .flat_map(|name| declared.pending_secret_keys(name, schema))
        .collect();
    let needed = |binding: Option<&str>, key: &str| match binding {
        Some(name) => in_use.contains(name),
        None => base_read.contains(&schema.canonical(key)),
    };

    let mut requests: BTreeMap<LayerSource<'_>, (&PluginId, BTreeSet<&SecretKey>)> =
        BTreeMap::new();
    for (binding, key, secret) in declared.plugin_secrets() {
        if !needed(binding, key) {
            continue;
        }
        if !secret.grant.admits(workload) {
            tracing::warn!(
                plugin_id,
                binding,
                secret = %secret.source,
                grant = %secret.grant,
                workload = %format!("{}/{}", workload.namespace, workload.name),
                "refused a bind reading a secret outside its grant"
            );
            let place = match binding {
                Some(name) => format!("binding `{name}` of `{plugin_id}`"),
                None => format!("`{plugin_id}`"),
            };
            anyhow::bail!(
                "{place} reads a secret that is not granted to workload `{}/{}`",
                workload.namespace,
                workload.name
            );
        }
        requests
            .entry((binding, secret.source.as_str()))
            .or_insert_with(|| (&secret.plugin, BTreeSet::new()))
            .1
            .insert(&secret.key);
    }

    let calls = requests
        .into_iter()
        .map(|((binding, source), (plugin, keys))| {
            let plugin_id = plugin_id.as_str();
            async move {
                let place = match binding {
                    Some(name) => format!("binding `{name}` of `{plugin_id}`"),
                    None => format!("`{plugin_id}`"),
                };
                let resolver = plugins
                    .get(plugin.as_str())
                    .and_then(|p| p.secret_source())
                    .with_context(|| {
                        format!(
                            "secret `{source}` for {place}: plugin `{plugin}` serves no \
                             secret-source"
                        )
                    })?;
                let resolved = resolver
                    .resolve(SecretRequest {
                        consumer: SecretConsumer {
                            plugin: plugin_id.into(),
                            binding: binding.map(BindingName::from),
                        },
                        workload: workload.clone(),
                        keys: keys.iter().map(|k| (*k).clone()).collect(),
                    })
                    .await
                    .and_then(|answer| {
                        check_answer(&keys, &answer)?;
                        Ok(answer)
                    })
                    .map_err(|e| {
                        anyhow::Error::new(e).context(format!(
                            "secret `{source}` for {place}: plugin `{plugin}` did not resolve it"
                        ))
                    })?;
                anyhow::Ok(((binding.map(str::to_string), source.to_string()), resolved))
            }
        });
    let values: HashMap<(Option<String>, String), BTreeMap<SecretKey, SecretValue>> =
        futures::future::try_join_all(calls)
            .await?
            .into_iter()
            .collect();

    let wanted: BTreeSet<String> = in_use.iter().map(|b| b.to_string()).collect();
    let mut used = Vec::new();
    let (set, refilled) = declared.fill_plugin_secrets(&wanted, |binding, key, secret| {
        if !needed(binding, key) {
            return Ok(None);
        }
        let value = values
            .get(&(binding.map(str::to_string), secret.source.to_string()))
            .and_then(|resolved| resolved.get(&secret.key))
            .cloned()
            .with_context(|| {
                format!(
                    "secret `{}` for `{plugin_id}`: plugin `{}` has no key `{}`",
                    secret.source, secret.plugin, secret.key
                )
            })?;
        used.push(value.clone());
        Ok(Some(value))
    })?;
    Ok(FilledSecrets {
        set,
        refilled,
        redactor: SecretRedactor(used),
    })
}

/// Holds a source's answer to the contract: every requested key, and no
/// other.
fn check_answer(
    requested: &BTreeSet<&SecretKey>,
    answer: &BTreeMap<SecretKey, SecretValue>,
) -> Result<(), SecretSourceError> {
    let named = |keys: Vec<&SecretKey>| {
        keys.into_iter()
            .map(|k| format!("`{k}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let missing: Vec<&SecretKey> = requested
        .iter()
        .copied()
        .filter(|k| !answer.contains_key(*k))
        .collect();
    let unrequested: Vec<&SecretKey> = answer.keys().filter(|k| !requested.contains(k)).collect();
    let mut faults = Vec::new();
    if !missing.is_empty() {
        faults.push(format!("left out {}", named(missing)));
    }
    if !unrequested.is_empty() {
        faults.push(format!("answered unrequested {}", named(unrequested)));
    }
    match faults.is_empty() {
        true => Ok(()),
        false => Err(SecretSourceError::InvalidAnswer(faults.join("; "))),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::plugin::bindings::PluginSecretRef;
    use crate::wit::WitWorld;

    /// Answers from a fixed map, recording each request.
    #[derive(Default)]
    struct Fixed(
        HashMap<&'static str, &'static str>,
        Mutex<Vec<SecretRequest>>,
    );

    #[async_trait::async_trait]
    impl HostSecretSource for Fixed {
        async fn resolve(
            &self,
            request: SecretRequest,
        ) -> Result<BTreeMap<SecretKey, SecretValue>, SecretSourceError> {
            if request.keys.iter().any(|k| k.as_str() == "denied") {
                return Err(SecretSourceError::AccessDenied(Some("not for you".into())));
            }
            let missing: Vec<SecretKey> = request
                .keys
                .iter()
                .filter(|k| !self.0.contains_key(k.as_str()))
                .cloned()
                .collect();
            if !missing.is_empty() {
                return Err(SecretSourceError::NotFound(missing));
            }
            let mut found: BTreeMap<SecretKey, SecretValue> = request
                .keys
                .iter()
                .map(|k| (k.clone(), SecretValue::new(self.0[k.as_str()])))
                .collect();
            if found.remove(&SecretKey::from("silent")).is_some() {
                found.insert("chatty".into(), SecretValue::new("extra"));
            }
            self.1.lock().unwrap().push(request);
            Ok(found)
        }
    }

    /// A plugin that serves nothing but may serve secrets to the host.
    struct Stub(&'static str, Option<Arc<Fixed>>);

    #[async_trait::async_trait]
    impl HostPlugin for Stub {
        fn id(&self) -> &'static str {
            self.0
        }
        fn world(&self) -> WitWorld {
            WitWorld::default()
        }
        fn secret_source(&self) -> Option<Arc<dyn HostSecretSource>> {
            self.1.clone().map(|r| r as Arc<dyn HostSecretSource>)
        }
    }

    fn plugins(source: Arc<Fixed>) -> HashMap<&'static str, Arc<dyn HostPlugin>> {
        HashMap::from([
            (
                "vault",
                Arc::new(Stub("vault", Some(source))) as Arc<dyn HostPlugin>,
            ),
            (
                "wasmcloud-postgres",
                Arc::new(Stub("wasmcloud-postgres", None)) as Arc<dyn HostPlugin>,
            ),
        ])
    }

    fn fixed() -> Arc<Fixed> {
        Arc::new(Fixed(
            HashMap::from([
                ("pg-url-app", "postgres://app:pw@db/app"),
                ("pg-url-reports", "postgres://reports:pw@db/app"),
                ("silent", "never sent"),
            ]),
            Mutex::default(),
        ))
    }

    fn secret(source: &str, key: &str) -> PluginSecretRef {
        PluginSecretRef {
            source: source.into(),
            plugin: "vault".into(),
            key: key.into(),
            grant: WorkloadGrant::Only(vec![WorkloadSelector::namespace("billing")]),
        }
    }

    fn requesting() -> RequestingWorkload {
        RequestingWorkload {
            id: "wl-1".to_string(),
            name: "billing-api".to_string(),
            namespace: "billing".to_string(),
        }
    }

    fn postgres(name: &str) -> WitInterface {
        let mut i = WitInterface::from("wasmcloud:postgres/query@0.2.0");
        i.name = Some(name.to_string());
        i
    }

    /// Two bindings, each reading its own secret, the way an operator writes
    /// them.
    fn declared() -> PluginBindingSet {
        PluginBindingSet::new("wasmcloud-postgres")
            .with_binding("app-db", HashMap::new())
            .with_binding_secrets(
                "app-db",
                HashMap::from([("url".to_string(), secret("app", "pg-url-app"))]),
            )
            .with_binding("reports-db", HashMap::new())
            .with_binding_secrets(
                "reports-db",
                HashMap::from([("url".to_string(), secret("reports", "pg-url-reports"))]),
            )
    }

    /// Only the bindings the workload binds are resolved, each with one call
    /// naming the consumer and binding; the filled set then resolves like
    /// one whose `url` was written inline.
    #[tokio::test]
    async fn only_the_bindings_in_use_are_resolved() {
        let source = fixed();
        let FilledSecrets {
            set: filled,
            refilled,
            ..
        } = resolve_plugin_secrets(
            &plugins(source.clone()),
            &requesting(),
            declared(),
            &HashSet::from([postgres("app-db")]),
            &BindingSchema::empty(),
        )
        .await
        .unwrap();
        // Only the filled binding is checked again, so a bad unrelated one
        // cannot fail this bind.
        let refilled = refilled.unwrap();
        assert_eq!(refilled, BTreeSet::from(["app-db".to_string()]));
        assert_eq!(
            filled
                .only_bindings(&refilled)
                .binding_names()
                .collect::<Vec<_>>(),
            ["app-db"]
        );
        assert_eq!(
            filled.host_layer("app-db")["url"],
            "postgres://app:pw@db/app"
        );
        let requests = source.1.lock().unwrap();
        assert_eq!(
            *requests,
            [SecretRequest {
                consumer: SecretConsumer {
                    plugin: "wasmcloud-postgres".into(),
                    binding: Some("app-db".into()),
                },
                workload: requesting(),
                keys: vec!["pg-url-app".into()],
            }]
        );
        let schema = BindingSchema::empty();
        assert!(
            filled
                .resolve("app-db", &HashMap::new(), &schema, &|_, _, _| false)
                .is_ok()
        );
        let err = filled
            .resolve("reports-db", &HashMap::new(), &schema, &|_, _, _| false)
            .unwrap_err();
        assert!(format!("{err}").contains("not resolved"), "{err}");
    }

    /// A workload outside an entry's grant fails the bind naming the binding
    /// and the workload, never the entry or its grant, and the source is never
    /// asked.
    #[tokio::test]
    async fn a_workload_outside_the_grant_is_refused_before_the_source_is_asked() {
        let only = |selector: WorkloadSelector| {
            let mut r = secret("app", "pg-url-app");
            r.grant = WorkloadGrant::Only(vec![selector]);
            PluginBindingSet::new("wasmcloud-postgres")
                .with_binding("app-db", HashMap::new())
                .with_binding_secrets("app-db", HashMap::from([("url".to_string(), r)]))
        };
        for (selector, admitted) in [
            (WorkloadSelector::namespace("billing"), true),
            (WorkloadSelector::workload("billing", "billing-api"), true),
            (WorkloadSelector::workload("billing", "reports"), false),
            (WorkloadSelector::namespace("other"), false),
        ] {
            let source = fixed();
            let result = resolve_plugin_secrets(
                &plugins(source.clone()),
                &requesting(),
                only(selector.clone()),
                &HashSet::from([postgres("app-db")]),
                &BindingSchema::empty(),
            )
            .await;
            assert_eq!(result.is_ok(), admitted, "{selector:?}");
            let asked = source.1.lock().unwrap().len();
            assert_eq!(asked, usize::from(admitted), "{selector:?}");
            if let Err(err) = result {
                let msg = format!("{err:#}");
                assert!(
                    msg.contains("binding `app-db`") && msg.contains("`billing/billing-api`"),
                    "{msg}"
                );
                let grant = WorkloadGrant::Only(vec![selector.clone()]).to_string();
                assert!(!msg.contains("`app`") && !msg.contains(&grant), "{msg}");
            }
        }
    }

    /// A key the source does not have fails the bind, naming the source and
    /// key.
    #[tokio::test]
    async fn a_missing_key_names_the_source_and_key() {
        let set = PluginBindingSet::new("wasmcloud-postgres")
            .with_binding("app-db", HashMap::new())
            .with_binding_secrets(
                "app-db",
                HashMap::from([("url".to_string(), secret("app", "nope"))]),
            );
        let err = resolve_plugin_secrets(
            &plugins(fixed()),
            &requesting(),
            set,
            &HashSet::from([postgres("app-db")]),
            &BindingSchema::empty(),
        )
        .await
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("`app`") && msg.contains("`nope`"), "{msg}");
    }

    /// An answer that leaves out a requested key or adds one nobody asked for
    /// fails the bind as an invalid answer.
    #[tokio::test]
    async fn an_answer_must_name_exactly_the_requested_keys() {
        let set = PluginBindingSet::new("wasmcloud-postgres")
            .with_binding("app-db", HashMap::new())
            .with_binding_secrets(
                "app-db",
                HashMap::from([("url".to_string(), secret("app", "silent"))]),
            );
        let err = resolve_plugin_secrets(
            &plugins(fixed()),
            &requesting(),
            set,
            &HashSet::from([postgres("app-db")]),
            &BindingSchema::empty(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<SecretSourceError>(),
                Some(SecretSourceError::InvalidAnswer(detail))
                    if detail == "left out `silent`; answered unrequested `chatty`"
            ),
            "{err:#}"
        );
    }

    /// One source on the entry is one call per bind, however many keys and
    /// bindings read it.
    #[tokio::test]
    async fn an_entry_source_is_resolved_once_per_bind() {
        let source = fixed();
        let set = declared().with_base_secrets(HashMap::from([
            ("url".to_string(), secret("shared", "pg-url-app")),
            (
                "fallback-url".to_string(),
                secret("shared", "pg-url-reports"),
            ),
        ]));
        let FilledSecrets { refilled, .. } = resolve_plugin_secrets(
            &plugins(source.clone()),
            &requesting(),
            set,
            &HashSet::from([postgres("app-db"), postgres("reports-db")]),
            &BindingSchema::empty(),
        )
        .await
        .unwrap();
        assert_eq!(
            refilled,
            Some(BTreeSet::from([
                "app-db".to_string(),
                "reports-db".to_string()
            ]))
        );
        let requests = source.1.lock().unwrap();
        let shared: Vec<_> = requests
            .iter()
            .filter(|r| r.consumer.binding.is_none())
            .collect();
        assert_eq!(shared.len(), 1);
        assert_eq!(
            shared[0].keys,
            [
                SecretKey::from("pg-url-app"),
                SecretKey::from("pg-url-reports")
            ]
        );
    }

    #[test]
    fn host_start_refuses_a_secret_that_could_never_resolve() {
        let plugins = plugins(fixed());
        let schema = BindingSchema::empty().and_host_ceiling_keys(["database"]);
        let check = |id: &str, key: &str, plugin: &str| {
            let mut r = secret("s", "k");
            r.plugin = plugin.into();
            let set = PluginBindingSet::new(id)
                .with_binding("app-db", HashMap::new())
                .with_binding_secrets("app-db", HashMap::from([(key.to_string(), r)]));
            validate_plugin_secrets(id, &set, &schema, &plugins).map_err(|e| e.to_string())
        };
        check("wasmcloud-postgres", "url", "vault").unwrap();
        for (id, key, plugin, expect) in [
            ("wasmcloud-postgres", "url", "nope", "not registered"),
            (
                "wasmcloud-postgres",
                "url",
                "wasmcloud-postgres",
                "its own config",
            ),
            ("wasmcloud-postgres", "database", "vault", "ceiling"),
        ] {
            let err = check(id, key, plugin).unwrap_err();
            assert!(err.contains(expect) && err.contains("`app-db`"), "{err}");
        }
    }

    /// A source's refusal reaches the bind failure as its typed case.
    #[tokio::test]
    async fn a_refusal_stays_typed() {
        let set = PluginBindingSet::new("wasmcloud-postgres")
            .with_binding("app-db", HashMap::new())
            .with_binding_secrets(
                "app-db",
                HashMap::from([("url".to_string(), secret("app", "denied"))]),
            );
        let err = resolve_plugin_secrets(
            &plugins(fixed()),
            &requesting(),
            set,
            &HashSet::from([postgres("app-db")]),
            &BindingSchema::empty(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                err.downcast_ref::<SecretSourceError>(),
                Some(SecretSourceError::AccessDenied(Some(msg))) if msg == "not for you"
            ),
            "{err:#}"
        );
    }

    /// Before a bind fills it, a pending secret's key is absent from the layer
    /// the plugin checks and named beside it, and its binding does not
    /// resolve.
    #[test]
    fn a_pending_secret_is_named_but_not_resolved() {
        let set = declared();
        let schema = BindingSchema::empty();
        let layers: BTreeMap<_, _> = set
            .host_layers(&schema)
            .map(|(name, layer, pending)| (name, (layer, pending)))
            .collect();
        assert_eq!(layers.len(), 2);
        let (layer, pending) = &layers["app-db"];
        assert!(!layer.contains_key("url"));
        assert_eq!(*pending, BTreeSet::from(["url".to_string()]));
        assert!(
            set.resolve("app-db", &HashMap::new(), &schema, &|_, _, _| false)
                .is_err()
        );
    }

    /// At host start a pending secret's key is judged by name: a typo is
    /// unknown, and a key also set in config, in any spelling, is refused.
    #[test]
    fn host_start_judges_pending_secret_keys_by_name() {
        let schema = BindingSchema::with_host_owned_keys(["url", "username"])
            .and_workload_owned_keys(Vec::<&str>::new())
            .and_aliases([("username", "user")]);
        let set = |config: &[(&str, &str)], key: &str| {
            PluginBindingSet::new("wasmcloud-postgres")
                .with_binding(
                    "app-db",
                    config
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                )
                .with_binding_secrets(
                    "app-db",
                    HashMap::from([(key.to_string(), secret("app", "pg-url-app"))]),
                )
        };
        set(&[], "url").reject_unknown_keys(&schema).unwrap();
        let err = set(&[], "ur1").reject_unknown_keys(&schema).unwrap_err();
        assert!(format!("{err}").contains("`ur1`"), "{err}");
        let err = set(&[("user", "svc")], "username")
            .reject_unknown_keys(&schema)
            .unwrap_err();
        assert!(format!("{err}").contains("secret `app`"), "{err}");
    }

    /// An entry-level secret is resolved only if some binding in use reads
    /// it; the unnamed binding always does, and its fill re-checks the entry.
    #[tokio::test]
    async fn an_entry_secret_every_binding_overrides_is_not_resolved() {
        let set = || {
            PluginBindingSet::new("wasmcloud-postgres")
                .with_base_secrets(HashMap::from([(
                    "url".to_string(),
                    secret("shared", "denied"),
                )]))
                .with_binding(
                    "app-db",
                    HashMap::from([("url".to_string(), "postgres://own".to_string())]),
                )
        };
        let schema = BindingSchema::empty();
        let filled = resolve_plugin_secrets(
            &plugins(fixed()),
            &requesting(),
            set(),
            &HashSet::from([postgres("app-db")]),
            &schema,
        )
        .await
        .expect("the refusing source is never asked");
        assert_eq!(filled.refilled, None);
        assert_eq!(filled.set.host_layer("app-db")["url"], "postgres://own");

        let mut unnamed = WitInterface::from("wasmcloud:postgres/query@0.2.0");
        unnamed.name = None;
        let err = resolve_plugin_secrets(
            &plugins(fixed()),
            &requesting(),
            set(),
            &HashSet::from([unnamed]),
            &schema,
        )
        .await
        .unwrap_err();
        assert!(err.downcast_ref::<SecretSourceError>().is_some(), "{err:#}");
    }

    /// A plugin parser quoting a filled value has it scrubbed.
    #[tokio::test]
    async fn an_error_quoting_a_filled_value_is_redacted() {
        let filled = resolve_plugin_secrets(
            &plugins(fixed()),
            &requesting(),
            declared(),
            &HashSet::from([postgres("app-db")]),
            &BindingSchema::empty(),
        )
        .await
        .unwrap();
        let err = filled
            .redactor
            .redact(anyhow::anyhow!("bad url `postgres://app:pw@db/app`"));
        assert_eq!(format!("{err:#}"), "bad url `<redacted>`");
    }

    #[test]
    fn a_secret_value_is_redacted() {
        let value = SecretValue::new("postgres://app:pw@db/app");
        assert_eq!(format!("{value:?}"), "SecretValue(<redacted>)");
        assert_eq!(value.expose(), Some("postgres://app:pw@db/app"));
    }
}
