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
use super::bindings::{BindingSchema, KeyOwnership, PluginBindingSet, UNNAMED_BINDING, binding_of};
use crate::wit::WitInterface;

/// What the host asks a [`HostSecretSource`] for, and on whose behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRequest {
    /// The plugin the values are for.
    pub consumer: String,
    /// The binding of `consumer` the source is declared on, or `None` for the
    /// entry itself.
    pub binding: Option<String>,
    /// The keys to resolve, without duplicates.
    pub keys: Vec<String>,
}

/// A secret's value. Redacted in `Debug`, with no `Display`, so reading it is
/// always an explicit [`Self::expose`].
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value itself. Never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// The value itself, for handing to its consumer. Never log it.
    pub fn into_exposed(self) -> String {
        self.0
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
    /// The source refuses these keys to this consumer.
    AccessDenied(Option<String>),
    /// The backend could not be reached; a later bind may succeed.
    Unavailable(Option<String>),
    /// Any other failure the source reported.
    Other(Option<String>),
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
            Self::Call(e) => return write!(f, "the call failed: {e:#}"),
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
    /// The value of each key in `request.keys`, keyed by it. A key left out was
    /// not found.
    async fn resolve(
        &self,
        request: SecretRequest,
    ) -> Result<HashMap<String, SecretValue>, SecretSourceError>;
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
            secret.plugin != plugin_id,
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

/// `declared` with the plugin secrets of every binding in `interfaces` filled
/// in. Each source declared on a layer is resolved with one call, keys
/// deduplicated, however many keys and entries read it.
///
/// # Errors
///
/// A source that is unavailable, refuses, or leaves a key out. Errors name the
/// entry, source and key, never a value; a source's own refusal is a
/// [`SecretSourceError`] in the chain.
pub(crate) async fn resolve_plugin_secrets(
    plugins: &HashMap<&'static str, Arc<dyn HostPlugin>>,
    declared: PluginBindingSet,
    interfaces: &HashSet<WitInterface>,
) -> anyhow::Result<PluginBindingSet> {
    if !declared.has_plugin_secrets() {
        return Ok(declared);
    }
    let plugin_id = declared.plugin_id().to_string();
    let in_use: BTreeSet<&str> = interfaces
        .iter()
        .map(|i| binding_of(&plugin_id, i.name.as_deref()).unwrap_or(UNNAMED_BINDING))
        .collect();

    let mut requests: BTreeMap<LayerSource<'_>, (&str, BTreeSet<&str>)> = BTreeMap::new();
    for (binding, _, secret) in declared.plugin_secrets() {
        if binding.is_some_and(|b| !in_use.contains(b)) {
            continue;
        }
        requests
            .entry((binding, secret.source.as_str()))
            .or_insert_with(|| (secret.plugin.as_str(), BTreeSet::new()))
            .1
            .insert(secret.key.as_str());
    }

    let mut values: HashMap<(Option<String>, String), HashMap<String, SecretValue>> =
        HashMap::new();
    for ((binding, source), (plugin, keys)) in requests {
        let place = match binding {
            Some(name) => format!("binding `{name}` of `{plugin_id}`"),
            None => format!("`{plugin_id}`"),
        };
        let resolver = plugins
            .get(plugin)
            .and_then(|p| p.secret_source())
            .with_context(|| {
                format!("secret `{source}` for {place}: plugin `{plugin}` serves no secret-source")
            })?;
        let resolved = resolver
            .resolve(SecretRequest {
                consumer: plugin_id.clone(),
                binding: binding.map(str::to_string),
                keys: keys.iter().map(|k| k.to_string()).collect(),
            })
            .await
            .map_err(|e| {
                anyhow::Error::new(e).context(format!(
                    "secret `{source}` for {place}: plugin `{plugin}` did not resolve it"
                ))
            })?;
        values.insert((binding.map(str::to_string), source.to_string()), resolved);
    }

    declared
        .clone()
        .fill_plugin_secrets(&in_use, |binding, secret| {
            values
                .get(&(binding.map(str::to_string), secret.source.clone()))
                .and_then(|resolved| resolved.get(&secret.key))
                .cloned()
                .with_context(|| {
                    format!(
                        "secret `{}` for `{plugin_id}`: plugin `{}` has no key `{}`",
                        secret.source, secret.plugin, secret.key
                    )
                })
        })
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
        ) -> Result<HashMap<String, SecretValue>, SecretSourceError> {
            if request.keys.iter().any(|k| k == "denied") {
                return Err(SecretSourceError::AccessDenied(Some("not for you".into())));
            }
            let found = request
                .keys
                .iter()
                .filter_map(|k| Some((k.clone(), SecretValue::new(*self.0.get(k.as_str())?))))
                .collect();
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
            ]),
            Mutex::default(),
        ))
    }

    fn secret(source: &str, key: &str) -> PluginSecretRef {
        PluginSecretRef {
            source: source.to_string(),
            plugin: "vault".to_string(),
            key: key.to_string(),
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
        let filled = resolve_plugin_secrets(
            &plugins(source.clone()),
            declared(),
            &HashSet::from([postgres("app-db")]),
        )
        .await
        .unwrap();
        assert_eq!(
            filled.host_layer("app-db")["url"],
            "postgres://app:pw@db/app"
        );
        let requests = source.1.lock().unwrap();
        assert_eq!(
            *requests,
            [SecretRequest {
                consumer: "wasmcloud-postgres".to_string(),
                binding: Some("app-db".to_string()),
                keys: vec!["pg-url-app".to_string()],
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

    /// A key the source leaves out fails the bind, naming the source and key.
    #[tokio::test]
    async fn a_missing_key_names_the_source_and_key() {
        let set = PluginBindingSet::new("wasmcloud-postgres")
            .with_binding("app-db", HashMap::new())
            .with_binding_secrets(
                "app-db",
                HashMap::from([("url".to_string(), secret("app", "nope"))]),
            );
        let err =
            resolve_plugin_secrets(&plugins(fixed()), set, &HashSet::from([postgres("app-db")]))
                .await
                .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("`app`") && msg.contains("`nope`"), "{msg}");
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
        resolve_plugin_secrets(
            &plugins(source.clone()),
            set,
            &HashSet::from([postgres("app-db"), postgres("reports-db")]),
        )
        .await
        .unwrap();
        let requests = source.1.lock().unwrap();
        let shared: Vec<_> = requests.iter().filter(|r| r.binding.is_none()).collect();
        assert_eq!(shared.len(), 1);
        assert_eq!(shared[0].keys, ["pg-url-app", "pg-url-reports"]);
    }

    #[test]
    fn host_start_refuses_a_secret_that_could_never_resolve() {
        let plugins = plugins(fixed());
        let schema = BindingSchema::empty().and_host_ceiling_keys(["database"]);
        let check = |id: &str, key: &str, plugin: &str| {
            let mut r = secret("s", "k");
            r.plugin = plugin.to_string();
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
        let err =
            resolve_plugin_secrets(&plugins(fixed()), set, &HashSet::from([postgres("app-db")]))
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

    /// Before a bind fills it, a pending secret's binding is not handed to the
    /// plugin's own checks, and does not resolve.
    #[test]
    fn a_pending_secret_is_neither_validated_nor_resolved() {
        let set = declared();
        let schema = BindingSchema::empty();
        assert_eq!(set.host_layers(&schema).count(), 0);
        assert!(!set.host_layer("app-db").contains_key("url"));
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

    #[test]
    fn a_secret_value_is_redacted() {
        let value = SecretValue::new("postgres://app:pw@db/app");
        assert_eq!(format!("{value:?}"), "SecretValue(<redacted>)");
        assert_eq!(value.expose(), "postgres://app:pw@db/app");
    }
}
