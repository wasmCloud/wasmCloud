//! Native `wasmcloud:secrets` plugin.
//!
//! Serves secrets from each component's bind-time `wasmcloud:secrets`
//! interface config — the platform sources those values from `secretFrom`
//! (or, locally, `dev.host_interfaces`) before the workload deploys. This
//! plugin does not itself talk to an external secrets backend; it is the
//! delivery mechanism for whatever the platform already resolved.
//!
//! Values leave only as opaque `secret` resource handles the caller unwraps
//! with `reveal`. Bindings are captured per component (`on_workload_item_bind`,
//! keyed by `(workload_id, component_id)`), matching [`crate::plugin::wasi_config::DynamicConfig`]'s
//! per-component isolation: two components in the same workload with
//! different `secretFrom` bindings never see each other's values, even
//! though `wasmcloud:secrets/store.get` carries no explicit caller argument
//! (it's ambient — resolved from the calling store's own identity). A
//! workload with exactly one bound component (including a host component
//! plugin resolving its own imports — see `component_host::link_native_imports`,
//! whose synthetic bind-time identity doesn't carry through to the plugin's
//! own runtime store) falls back to that single entry regardless of the
//! caller's exact component id, since there is nothing to disambiguate.
//!
//! `wasmcloud:secrets` is a native-async WIT interface (`async func`, not
//! sync-func-with-an-async-Rust-trait), so the generated host bindings use
//! wasmtime's concurrent ABI: the real logic lives on `HostWithStore<T> for
//! SharedCtx` (free functions taking an `Accessor`, not `&self` methods),
//! mirroring `wasi_keyvalue::multiplexed_async`.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::instrument;
use wasmtime::component::{Accessor, Resource};
use zeroize::Zeroize;

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};

/// The resolved value behind one labeled `wasmcloud:secrets/secret` import,
/// looked up once per label at bind time (see `named_imports` below) and
/// cloned per call. A plain alias, not `Arc<str>` spelled out inline in the
/// macro — `named_imports` splices this string into a generated module path,
/// which can't contain `<`/`>`.
pub type SecretRef = Arc<str>;

mod bindings {
    wasmtime::component::bindgen!({
        world: "secrets",
        imports: { default: async | trappable | tracing },
        named_imports: {
            "wasmcloud:secrets/secret": crate::plugin::wasmcloud_secrets::SecretRef,
        },
        with: {
            "wasmcloud:secrets/store.secret": crate::plugin::wasmcloud_secrets::SecretState,
        },
    });
}

use bindings::wasmcloud::secrets::store::{SecretValue, SecretsError};

const WASMCLOUD_SECRETS_ID: &str = "wasmcloud-secrets";

/// Backing state for the exported `secret` resource: the resolved value, kept
/// opaque to callers until they `reveal` it.
pub struct SecretState {
    value: SecretValue,
}

impl Drop for SecretState {
    /// Each `get` resolves a fresh copy of the value into a new `SecretState`;
    /// scrub it when the resource is dropped so a revealed secret doesn't
    /// linger in freed heap past the call that resolved it.
    fn drop(&mut self) {
        match &mut self.value {
            SecretValue::String(s) => s.zeroize(),
            SecretValue::Bytes(b) => b.zeroize(),
        }
    }
}

type ComponentSecrets = BTreeMap<String, String>;

/// Native `wasmcloud:secrets` plugin. Bind-time secret configuration, keyed
/// by workload id then by component id.
#[derive(Clone, Default)]
pub struct WasmcloudSecrets {
    binds: Arc<RwLock<BTreeMap<String, BTreeMap<String, ComponentSecrets>>>>,
}

impl WasmcloudSecrets {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Zeroizes every value in a captured component's secret config before it's
/// dropped, whether because the component unbound or a rebind replaced it.
fn zeroize_config(config: &mut ComponentSecrets) {
    for value in config.values_mut() {
        value.zeroize();
    }
}

/// `secret-value` does not derive `Clone` in the generated bindings, so rebuild
/// it by variant to return an owned copy.
fn clone_value(value: &SecretValue) -> SecretValue {
    match value {
        SecretValue::String(s) => SecretValue::String(s.clone()),
        SecretValue::Bytes(b) => SecretValue::Bytes(b.clone()),
    }
}

/// Resolves `key` for the calling `(workload_id, component_id)`: an exact
/// per-component match when there's genuine ambiguity to resolve (more than
/// one component ever bound under this workload id), or the workload's sole
/// bound component's config when there's only one — see the module doc for
/// why that single-entry fallback is needed and why it never weakens
/// isolation between components that actually coexist.
fn resolve<'a>(
    binds: &'a BTreeMap<String, ComponentSecrets>,
    component_id: &str,
) -> Option<&'a ComponentSecrets> {
    binds.get(component_id).or_else(|| {
        if binds.len() == 1 {
            binds.values().next()
        } else {
            None
        }
    })
}

impl<T: 'static + Send> bindings::wasmcloud::secrets::store::HostWithStore<T> for SharedCtx {
    /// Resolve `key` from the calling component's bind-time secret config. A
    /// missing key is `not-found`.
    #[instrument(name = "wasmcloud.secrets.get", skip(accessor))]
    async fn get(
        accessor: &Accessor<T, Self>,
        key: String,
    ) -> wasmtime::Result<Result<Resource<SecretState>, SecretsError>> {
        let (plugin, workload_id, component_id) = accessor.with(|mut access| {
            let ctx = access.get();
            wasmtime::Result::<(Arc<WasmcloudSecrets>, Arc<str>, Arc<str>)>::Ok((
                ctx.try_get_plugin::<WasmcloudSecrets>(WASMCLOUD_SECRETS_ID)?,
                ctx.workload_id.clone(),
                ctx.component_id.clone(),
            ))
        })?;
        let value = {
            let binds = plugin.binds.read().await;
            binds
                .get(&*workload_id)
                .and_then(|components| resolve(components, &component_id))
                .and_then(|config| config.get(&key).cloned())
        };
        match value {
            Some(value) => {
                let resource = accessor.with(|mut access| {
                    access.get().table.push(SecretState {
                        value: SecretValue::String(value),
                    })
                })?;
                Ok(Ok(resource))
            }
            None => Ok(Err(SecretsError::NotFound)),
        }
    }
}

impl bindings::wasmcloud::secrets::store::Host for ActiveCtx<'_> {}

impl bindings::wasmcloud::secrets::store::HostSecret for ActiveCtx<'_> {
    async fn drop(&mut self, rep: Resource<SecretState>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl<T: 'static + Send> bindings::wasmcloud::secrets::reveal::HostWithStore<T> for SharedCtx {
    #[instrument(name = "wasmcloud.secrets.reveal", skip(accessor, secret))]
    async fn reveal(
        accessor: &Accessor<T, Self>,
        secret: Resource<SecretState>,
    ) -> wasmtime::Result<SecretValue> {
        accessor.with(|mut access| {
            let state = access.get().table.get(&secret)?;
            wasmtime::Result::Ok(clone_value(&state.value))
        })
    }
}

impl bindings::wasmcloud::secrets::reveal::Host for ActiveCtx<'_> {}

/// Serves a labeled `wasmcloud:secrets/secret` import's nullary `get()`: the
/// resolved value for this specific label, looked up once per label when the
/// bind's `add_to_linker` call is made (see [`WasmcloudSecrets::on_workload_item_bind`]),
/// then cloned per call — never a per-call lookup keyed by a guest-supplied
/// string, since there is no such string in this interface.
///
/// Unlike `store`'s `binds` map, the resolved value baked in here has no
/// zeroize-on-unbind path: it lives inside the linker's per-label closure for
/// as long as the component's `InstancePre` does — for a host component
/// plugin, that spans every incarnation, not just one bind — rather than
/// just while the bind is active. Only the `SecretState` resource `get`
/// allocates fresh per call is scrubbed on drop.
impl<T: 'static + Send> bindings::named_imports::wasmcloud::secrets::secret::HostWithStore<T>
    for SharedCtx
{
    #[instrument(name = "wasmcloud.secrets.secret.get", skip(accessor))]
    async fn get(
        accessor: &Accessor<T, Self>,
        id: SecretRef,
    ) -> wasmtime::Result<Resource<SecretState>> {
        let resource = accessor.with(|mut access| {
            access.get().table.push(SecretState {
                value: SecretValue::String(id.to_string()),
            })
        })?;
        Ok(resource)
    }
}

impl bindings::named_imports::wasmcloud::secrets::secret::Host for ActiveCtx<'_> {}

#[async_trait::async_trait]
impl HostPlugin for WasmcloudSecrets {
    fn id(&self) -> &'static str {
        WASMCLOUD_SECRETS_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([
                WitInterface::from("wasmcloud:secrets/store,reveal@2.0.0"),
                WitInterface::from("wasmcloud:secrets/secret@2.0.0"),
            ]),
            exports: HashSet::new(),
        }
    }

    /// A component may import several distinct labeled `wasmcloud:secrets/secret`
    /// instances (one per secret it needs) — the `(implements ..)` routing
    /// [`WasmcloudSecrets::on_workload_item_bind`] resolves each of against its
    /// own bind-time config entry.
    fn supports_named_instances(&self) -> bool {
        true
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        let has_dynamic = interfaces.contains("wasmcloud", "secrets", &["store", "reveal"]);

        // Every labeled `wasmcloud:secrets/secret` import: label -> the value
        // at that same key in the label's own resolved config, if present.
        // The label *is* the config key by convention — there's no separate
        // `key` argument for a labeled import to carry one.
        let mut labeled: BTreeMap<String, Option<String>> = interfaces
            .iter()
            .filter(|i| {
                i.namespace == "wasmcloud"
                    && i.package == "secrets"
                    && i.interfaces.contains("secret")
            })
            .filter_map(|i| {
                let label = i.name.clone()?;
                let value = i.config.get(&label).cloned();
                Some((label, value))
            })
            .collect();

        if !has_dynamic && labeled.is_empty() {
            return Ok(());
        }

        // The `secret` resource type (and its drop impl) is owned by `store`;
        // register it whenever anything below needs it, even for a
        // labeled-secret-only component that never imports plain `store`
        // itself — `secret` and `reveal` only `use` the type, they don't
        // define it.
        bindings::wasmcloud::secrets::store::add_to_linker::<_, SharedCtx>(
            item.linker(),
            extract_active_ctx,
        )?;

        if has_dynamic {
            // Flatten every matched *dynamic* (unlabeled `store`/`reveal`)
            // interface's config into one per-component map; the
            // `wasmcloud:secrets` binding's config carries the credentials
            // the platform sourced from `secretFrom`. `store.get` takes only
            // a key, with no way to say which binding it's asking on behalf
            // of, so a component with multiple `wasmcloud:secrets` bindings
            // that happen to declare the same key is unresolvable — reject
            // the bind rather than let one binding's value silently shadow
            // another's.
            let mut config = BTreeMap::new();
            for iface in interfaces.iter().filter(|i| i.name.is_none()) {
                for (key, value) in &iface.config {
                    if config.insert(key.clone(), value.clone()).is_some() {
                        anyhow::bail!(
                            "secret key {key:?} is set by more than one binding for component \
                             {:?} in workload {:?}",
                            item.id(),
                            item.workload_id()
                        );
                    }
                }
            }

            bindings::wasmcloud::secrets::reveal::add_to_linker::<_, SharedCtx>(
                item.linker(),
                extract_active_ctx,
            )?;

            // A rebind (e.g. the host replaying binds after a plugin restart)
            // replaces this component's entry; scrub whatever it replaced
            // instead of just letting it drop.
            let mut binds = self.binds.write().await;
            let components = binds.entry(item.workload_id().to_string()).or_default();
            if let Some(mut previous) = components.insert(item.id().to_string(), config) {
                zeroize_config(&mut previous);
            }
        }

        if !labeled.is_empty() {
            // A labeled-secret-only component (no unlabeled `store`/`reveal`
            // above) still needs `reveal` wired to unwrap its handles.
            if !has_dynamic {
                bindings::wasmcloud::secrets::reveal::add_to_linker::<_, SharedCtx>(
                    item.linker(),
                    extract_active_ctx,
                )?;
            }

            // Resolved once per label right here, from the component's own
            // import type — before any instantiation, let alone a call. A
            // label with no matching config entry aborts the bind with the
            // specific missing key named, the same enforcement moment as the
            // dynamic path's config collision above, just reached earlier:
            // from the import list alone, not a runtime `not-found`.
            let component = item.component().clone();
            let id: Arc<str> = Arc::from(item.id());
            // Borrows `labeled` rather than moving it in: `add_to_linker`
            // only calls `lookup` synchronously, inline, while resolving
            // each matched label — never after it returns — so `labeled`'s
            // plaintext values can be scrubbed right after, rather than
            // living on unzeroized for as long as this closure would.
            bindings::named_imports::wasmcloud::secrets::secret::add_to_linker::<_, SharedCtx>(
                item.linker(),
                &component,
                |label: &str| match labeled.get(label) {
                    Some(Some(value)) => Ok(Arc::<str>::from(value.as_str())),
                    _ => Err(wasmtime::Error::msg(format!(
                        "component {id:?} requires secret {label:?} but it was not provided in \
                         its resolved bind-time config"
                    ))),
                },
                extract_active_ctx,
            )?;
            for value in labeled.values_mut().flatten() {
                value.zeroize();
            }
        }

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        // The engine fans this out once per component that was bound to this
        // plugin, all carrying the same `workload_id` (it has no per-item
        // identity to give); a workload's components always stop together,
        // so scrubbing the whole workload entry on the first call and no-op'ing
        // on the rest is correct, not a race.
        //
        // Scrub the reclaimed credential strings rather than just dropping
        // them, so plaintext secrets don't linger in freed heap after the
        // workload they belonged to is gone.
        let mut binds = self.binds.write().await;
        if let Some(mut components) = binds.remove(workload_id) {
            for config in components.values_mut() {
                zeroize_config(config);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The security-relevant case this whole module exists to get right: two
    /// components of one workload, each with its own distinct
    /// `wasmcloud:secrets` binding, must not see each other's keys — even
    /// though `store.get` carries no explicit caller argument and both
    /// components share the same `workload_id`.
    #[test]
    fn resolve_isolates_components_with_distinct_bindings() {
        let mut binds: BTreeMap<String, ComponentSecrets> = BTreeMap::new();
        binds.insert(
            "component-a".to_string(),
            BTreeMap::from([("DB_PASSWORD".to_string(), "a-secret".to_string())]),
        );
        binds.insert(
            "component-b".to_string(),
            BTreeMap::from([("API_KEY".to_string(), "b-secret".to_string())]),
        );

        let a = resolve(&binds, "component-a").expect("component-a has its own entry");
        assert_eq!(a.get("DB_PASSWORD"), Some(&"a-secret".to_string()));
        assert_eq!(
            a.get("API_KEY"),
            None,
            "component-a must not see component-b's key"
        );

        let b = resolve(&binds, "component-b").expect("component-b has its own entry");
        assert_eq!(b.get("API_KEY"), Some(&"b-secret".to_string()));
        assert_eq!(
            b.get("DB_PASSWORD"),
            None,
            "component-b must not see component-a's key"
        );
    }

    /// A caller whose component id doesn't match anything bound (the
    /// scenario a host component plugin's own synthetic bind-time identity
    /// produces, since it never carries through to the plugin's own runtime
    /// store) falls back to the sole bound entry when there's exactly one —
    /// nothing else exists to disambiguate from, so this can't leak a
    /// neighboring component's secret.
    #[test]
    fn resolve_falls_back_to_the_sole_entry_for_an_unknown_component_id() {
        let mut binds: BTreeMap<String, ComponentSecrets> = BTreeMap::new();
        binds.insert(
            "some-random-uuid".to_string(),
            BTreeMap::from([("api-key".to_string(), "s3cr3t".to_string())]),
        );

        let resolved =
            resolve(&binds, "a-completely-different-caller-id").expect("single-entry fallback");
        assert_eq!(resolved.get("api-key"), Some(&"s3cr3t".to_string()));
    }

    /// The fallback must NOT apply once there is genuine ambiguity — an
    /// unknown component id against two-or-more bound components resolves to
    /// nothing, rather than guessing.
    #[test]
    fn resolve_does_not_fall_back_when_ambiguous() {
        let mut binds: BTreeMap<String, ComponentSecrets> = BTreeMap::new();
        binds.insert(
            "component-a".to_string(),
            BTreeMap::from([("k".to_string(), "a".to_string())]),
        );
        binds.insert(
            "component-b".to_string(),
            BTreeMap::from([("k".to_string(), "b".to_string())]),
        );

        assert!(resolve(&binds, "component-c").is_none());
    }
}
