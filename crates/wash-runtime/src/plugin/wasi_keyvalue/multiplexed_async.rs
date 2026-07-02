//! # Multiplexed async `wasmcloud:keyvalue` (implements-routed)
//!
//! The async-native counterpart to [`super::multiplexed`]. Binds
//! `wasmcloud:keyvalue@0.1.0` whose every backend operation is an `async func`
//! (no `wasi:io/poll`) via the same `(implements ..)` / `named_imports`
//! mechanism, routing each named import to a [`KvBackend`].
//!
//! Because the WIT methods are `async func`s, the generated host traits use
//! wasmtime's *concurrent* ABI: methods are `async fn`s taking an [`Accessor`].
//! They reuse the existing [`KvBackend`] backends (in-memory / redis / NATS /
//! filesystem); some of the richer async-only surface is mapped at this host
//! layer over the backend's operations:
//!
//! * **CAS** (`cas.current` / `cas.swap`) delegates to [`KvBackend::current`] /
//!   [`KvBackend::swap`], which perform an atomic compare-and-set against the
//!   backend using a backend-native, write-monotonic version (NATS revision,
//!   in-memory counter). Backends without an atomic primitive return an
//!   unsupported error rather than a racy host-side read-modify-write.
//! * **`set-options`**: `if-not-exists` is an `exists`-then-`set` check (racy);
//!   `ttl-ms` is rejected with an error, since the basic backends cannot honor
//!   expiry (the WIT requires erroring rather than silently ignoring a TTL).
//! * **`atomics.increment`** is a direct pass-through onto the backend's signed
//!   `increment` (the WIT delta/counter are `s64`; a negative delta decrements,
//!   unlike `wasi:keyvalue`'s unsigned counter).
//! * **`list-keys`** maps the opaque string cursor to the backend's `u64` cursor
//!   and filters by `prefix` host-side.

use std::collections::HashSet;
use std::sync::Arc;

use wasmtime::component::{Accessor, Resource};

use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::multiplex::Multiplexer;
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};

use super::multiplexed::{CasGuard, CasOutcome, KvBucket, KvId, KvProvider, StoreError, Versioned};

mod bindings {
    wasmtime::component::bindgen!({
        world: "async-keyvalue",
        imports: { default: async | trappable | tracing },
        // Only `store` is routed per `(implements ..)` label; `atomics`/`cas`/
        // `batch` are bound as standalone interfaces (they borrow the bucket,
        // which carries its backend) — see `on_workload_item_bind`.
        named_imports: {
            "wasmcloud:keyvalue/store": crate::plugin::wasi_keyvalue::multiplexed::KvId,
        },
        with: {
            "wasmcloud:keyvalue/types.bucket":
                crate::plugin::wasi_keyvalue::multiplexed::KvBucket,
        },
    });
}

use bindings::wasmcloud::keyvalue::cas::{CasOptions, CasResult, Entry};
use bindings::wasmcloud::keyvalue::types::{Error as AsyncKvError, KeyResponse, SetOptions};

const DEFAULT_BACKEND: &str = "in-memory";
const MULTIPLEXED_ASYNC_KEYVALUE_ID: &str = "wasmcloud-keyvalue-multiplexed";

impl From<StoreError> for AsyncKvError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NoSuchStore => AsyncKvError::NoSuchStore,
            StoreError::AccessDenied => AsyncKvError::AccessDenied,
            StoreError::Other(s) => AsyncKvError::Other(s),
        }
    }
}

/// Read `(backend, bucket-name)` out of a bucket resource without holding the
/// store borrow across an await.
fn bucket_ref<T>(
    access: &mut wasmtime::component::Access<'_, T, SharedCtx>,
    bucket: &Resource<KvBucket>,
) -> wasmtime::Result<(KvId, String)> {
    let b = access.get().table.get(bucket)?;
    Ok((b.backend().clone(), b.name().to_string()))
}

// The `bucket` resource lives in the `types` interface (shared by store/atomics/
// cas/batch), so its methods are bound standalone — the `KvBucket` carries the
// backend it was opened through. Only `store.open` is label-routed.
impl<T: 'static + Send> bindings::wasmcloud::keyvalue::types::HostBucketWithStore<T> for SharedCtx {
    async fn get(
        accessor: &Accessor<T, Self>,
        self_: Resource<KvBucket>,
        key: String,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, AsyncKvError>> {
        let (backend, name) = accessor.with(|mut a| bucket_ref(&mut a, &self_))?;
        Ok(backend.get(&name, &key).await.map_err(Into::into))
    }

    async fn set(
        accessor: &Accessor<T, Self>,
        self_: Resource<KvBucket>,
        key: String,
        value: Vec<u8>,
        options: Option<SetOptions>,
    ) -> wasmtime::Result<Result<(), AsyncKvError>> {
        let (backend, name) = accessor.with(|mut a| bucket_ref(&mut a, &self_))?;
        if let Some(opts) = &options {
            if opts.ttl_ms.is_some() {
                return Ok(Err(AsyncKvError::Other(
                    "ttl-ms is not supported by this backend".to_string(),
                )));
            }
            if opts.if_not_exists {
                // Atomic conditional insert (backend `put_if_absent`), not a
                // racy host-side `exists`-then-`set` — `precondition-failed`
                // when the key already exists.
                return Ok(match backend.put_if_absent(&name, &key, value).await {
                    Ok(true) => Ok(()),
                    Ok(false) => Err(AsyncKvError::PreconditionFailed),
                    Err(e) => Err(e.into()),
                });
            }
        }
        Ok(backend.set(&name, &key, value).await.map_err(Into::into))
    }

    async fn delete(
        accessor: &Accessor<T, Self>,
        self_: Resource<KvBucket>,
        key: String,
    ) -> wasmtime::Result<Result<(), AsyncKvError>> {
        let (backend, name) = accessor.with(|mut a| bucket_ref(&mut a, &self_))?;
        Ok(backend.delete(&name, &key).await.map_err(Into::into))
    }

    async fn exists(
        accessor: &Accessor<T, Self>,
        self_: Resource<KvBucket>,
        key: String,
    ) -> wasmtime::Result<Result<bool, AsyncKvError>> {
        let (backend, name) = accessor.with(|mut a| bucket_ref(&mut a, &self_))?;
        Ok(backend.exists(&name, &key).await.map_err(Into::into))
    }

    async fn list_keys(
        accessor: &Accessor<T, Self>,
        self_: Resource<KvBucket>,
        prefix: Option<String>,
        cursor: Option<String>,
    ) -> wasmtime::Result<Result<KeyResponse, AsyncKvError>> {
        let (backend, name) = accessor.with(|mut a| bucket_ref(&mut a, &self_))?;
        // A malformed cursor must be rejected, not silently treated as `None`
        // (start-from-the-beginning).
        let cursor = match cursor.as_deref() {
            None => None,
            Some(c) => match c.parse::<u64>() {
                Ok(n) => Some(n),
                Err(_) => return Ok(Err(AsyncKvError::InvalidArgument)),
            },
        };
        let resp = match backend.list_keys(&name, cursor).await {
            Ok(resp) => resp,
            Err(e) => return Ok(Err(e.into())),
        };
        let keys = match &prefix {
            Some(p) => resp.keys.into_iter().filter(|k| k.starts_with(p)).collect(),
            None => resp.keys,
        };
        Ok(Ok(KeyResponse {
            keys,
            cursor: resp.cursor.map(|c| c.to_string()),
        }))
    }
}

impl bindings::wasmcloud::keyvalue::types::HostBucket for ActiveCtx<'_> {
    async fn drop(&mut self, rep: Resource<KvBucket>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl bindings::wasmcloud::keyvalue::types::Host for ActiveCtx<'_> {}

// `store` keeps only `open`, routed per `(implements ..)` label.
impl<T: 'static + Send> bindings::named_imports::wasmcloud::keyvalue::store::HostWithStore<T>
    for SharedCtx
{
    async fn open(
        accessor: &Accessor<T, Self>,
        id: KvId,
        identifier: String,
    ) -> wasmtime::Result<Result<Resource<KvBucket>, AsyncKvError>> {
        if let Err(e) = id.open(&identifier).await {
            return Ok(Err(e.into()));
        }
        let resource = accessor.with(|mut a| a.get().table.push(KvBucket::new(id, identifier)))?;
        Ok(Ok(resource))
    }
}

impl bindings::named_imports::wasmcloud::keyvalue::store::Host for ActiveCtx<'_> {}

/// A plain (unlabeled) `store.open`: route to the workload's default backend
/// (recorded on the multiplexer at bind) so a component that imports
/// `wasmcloud:keyvalue/store` *without* an `(implements ..)` label still gets a
/// working backend — no label required. The label-routed `open` above is
/// identical but for taking its `KvId` from the label instead of the default.
impl<T: 'static + Send> bindings::wasmcloud::keyvalue::store::HostWithStore<T> for SharedCtx {
    async fn open(
        accessor: &Accessor<T, Self>,
        identifier: String,
    ) -> wasmtime::Result<Result<Resource<KvBucket>, AsyncKvError>> {
        let Some(id) = default_backend(accessor).await? else {
            return no_default_backend();
        };
        if let Err(e) = id.open(&identifier).await {
            return Ok(Err(e.into()));
        }
        let resource = accessor.with(|mut a| a.get().table.push(KvBucket::new(id, identifier)))?;
        Ok(Ok(resource))
    }
}

impl bindings::wasmcloud::keyvalue::store::Host for ActiveCtx<'_> {}

/// The workload's default `wasmcloud:keyvalue` backend for a PLAIN (unlabeled)
/// import, recorded on the multiplexer at bind. `None` when the workload
/// declared no default (`""`) route.
async fn default_backend<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
) -> wasmtime::Result<Option<KvId>> {
    let (plugin, workload_id) = accessor.with(|mut a| {
        let ctx = a.get();
        (
            ctx.try_get_plugin::<MultiplexedAsyncKeyValue>(MULTIPLEXED_ASYNC_KEYVALUE_ID),
            ctx.workload_id.clone(),
        )
    });
    let plugin = plugin?;
    Ok(plugin.mux.default_for(&workload_id))
}

/// The error a plain `store.open` returns when the workload bound no default
/// backend — the component imported keyvalue plainly but nothing provides it.
fn no_default_backend<T2>() -> wasmtime::Result<Result<T2, AsyncKvError>> {
    Ok(Err(AsyncKvError::Other(
        "no default wasmcloud:keyvalue backend is bound for this component".to_string(),
    )))
}

impl<T: 'static + Send> bindings::wasmcloud::keyvalue::atomics::HostWithStore<T> for SharedCtx {
    async fn increment(
        accessor: &Accessor<T, Self>,
        bucket: Resource<KvBucket>,
        key: String,
        delta: i64,
    ) -> wasmtime::Result<Result<i64, AsyncKvError>> {
        // `delta` and the returned counter are signed `s64` (a negative delta
        // decrements), so this is a direct pass-through onto the backend's signed
        // `increment`.
        let (backend, name) = accessor.with(|mut a| bucket_ref(&mut a, &bucket))?;
        Ok(backend
            .increment(&name, &key, delta)
            .await
            .map_err(Into::into))
    }
}

impl bindings::wasmcloud::keyvalue::atomics::Host for ActiveCtx<'_> {}

impl<T: 'static + Send> bindings::wasmcloud::keyvalue::cas::HostWithStore<T> for SharedCtx {
    async fn current(
        accessor: &Accessor<T, Self>,
        bucket: Resource<KvBucket>,
        key: String,
    ) -> wasmtime::Result<Result<Option<Entry>, AsyncKvError>> {
        let (backend, name) = accessor.with(|mut a| bucket_ref(&mut a, &bucket))?;
        Ok(backend
            .current(&name, &key)
            .await
            .map(|opt| opt.map(into_entry))
            .map_err(Into::into))
    }

    async fn swap(
        accessor: &Accessor<T, Self>,
        bucket: Resource<KvBucket>,
        key: String,
        value: Vec<u8>,
        options: CasOptions,
    ) -> wasmtime::Result<Result<CasResult, AsyncKvError>> {
        if options.require_version.is_none() && options.require_value.is_none() {
            return Ok(Err(AsyncKvError::InvalidArgument));
        }
        let (backend, name) = accessor.with(|mut a| bucket_ref(&mut a, &bucket))?;
        // The backend performs the compare-and-set atomically (NATS revision,
        // in-memory under one write lock), so there is no host-side
        // read-then-write race and the version check is ABA-safe.
        let guard = CasGuard {
            require_version: options.require_version,
            require_value: options.require_value,
        };
        Ok(backend
            .swap(&name, &key, value, guard)
            .await
            .map(|outcome| match outcome {
                CasOutcome::Swapped => CasResult::Swapped,
                CasOutcome::Stale(cur) => CasResult::Stale(cur.map(into_entry)),
            })
            .map_err(Into::into))
    }
}

/// Convert a backend [`Versioned`] into the WIT `cas.entry`.
fn into_entry(v: Versioned) -> Entry {
    Entry {
        value: v.value,
        version: v.version,
    }
}

impl bindings::wasmcloud::keyvalue::cas::Host for ActiveCtx<'_> {}

impl<T: 'static + Send> bindings::wasmcloud::keyvalue::batch::HostWithStore<T> for SharedCtx {
    #[allow(clippy::type_complexity)]
    async fn get_many(
        accessor: &Accessor<T, Self>,
        bucket: Resource<KvBucket>,
        keys: Vec<String>,
    ) -> wasmtime::Result<Result<Vec<Option<(String, Vec<u8>)>>, AsyncKvError>> {
        let (backend, name) = accessor.with(|mut a| bucket_ref(&mut a, &bucket))?;
        Ok(backend.get_many(&name, keys).await.map_err(Into::into))
    }

    async fn set_many(
        accessor: &Accessor<T, Self>,
        bucket: Resource<KvBucket>,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> wasmtime::Result<Result<(), AsyncKvError>> {
        let (backend, name) = accessor.with(|mut a| bucket_ref(&mut a, &bucket))?;
        Ok(backend
            .set_many(&name, key_values)
            .await
            .map_err(Into::into))
    }

    async fn delete_many(
        accessor: &Accessor<T, Self>,
        bucket: Resource<KvBucket>,
        keys: Vec<String>,
    ) -> wasmtime::Result<Result<(), AsyncKvError>> {
        let (backend, name) = accessor.with(|mut a| bucket_ref(&mut a, &bucket))?;
        Ok(backend.delete_many(&name, keys).await.map_err(Into::into))
    }
}

impl bindings::wasmcloud::keyvalue::batch::Host for ActiveCtx<'_> {}

/// A keyvalue [`HostPlugin`] that multiplexes async `wasmcloud:keyvalue` across
/// backends selected per `(implements ..)` import. Shares the [`KvBackend`]
/// providers with [`super::multiplexed::MultiplexedKeyValue`].
pub struct MultiplexedAsyncKeyValue {
    mux: Multiplexer<KvId>,
}

impl Default for MultiplexedAsyncKeyValue {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiplexedAsyncKeyValue {
    pub fn new() -> Self {
        Self {
            mux: Multiplexer::new("wasmcloud", "keyvalue", DEFAULT_BACKEND),
        }
    }

    /// Register a backend provider keyed by its `backend_type()`.
    pub fn with_provider(mut self, provider: Arc<KvProvider>) -> Self {
        self.mux = self.mux.with_provider(provider);
        self
    }

    /// Build the routing registry (host-interface name -> backend) for a
    /// component from its matched keyvalue host interfaces.
    pub async fn build_registry<'i>(
        &self,
        interfaces: impl IntoIterator<Item = &'i WitInterface>,
    ) -> anyhow::Result<std::collections::HashMap<String, KvId>> {
        self.mux.build_registry(interfaces).await
    }
}

#[async_trait::async_trait]
impl HostPlugin for MultiplexedAsyncKeyValue {
    fn id(&self) -> &'static str {
        MULTIPLEXED_ASYNC_KEYVALUE_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasmcloud:keyvalue/store,atomics,cas,batch@0.1.0",
            )]),
            ..Default::default()
        }
    }

    fn supports_named_instances(&self) -> bool {
        true
    }

    async fn on_workload_item_bind<'a>(
        &self,
        item: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        if !interfaces.contains("wasmcloud", "keyvalue", &[]) {
            return Ok(());
        }

        let registry = self.build_registry(interfaces.iter()).await?;

        // Does the component import keyvalue with an `(implements ..)` label, or
        // plainly (unlabeled), or both? Bind only the matching `store` binding so a
        // plain-only component doesn't also get the labeled instance (and vice
        // versa) — `types`/`atomics`/`cas`/`batch` are bound standalone regardless.
        let has_labeled = interfaces
            .iter()
            .any(|i| i.namespace == "wasmcloud" && i.package == "keyvalue" && i.name.is_some());
        let has_plain = interfaces
            .iter()
            .any(|i| i.namespace == "wasmcloud" && i.package == "keyvalue" && i.name.is_none());

        // A plain import routes to the workload's default backend (the `""` route
        // from the registry), stashed on the multiplexer so the standard host impl
        // can find it — the shared mechanism every multiplexed plugin uses.
        if has_plain {
            self.mux.set_default(item.workload_id(), &registry);
        }

        let component = item.component().clone();
        let linker = item.linker();

        // Only `store.open` is routed per `(implements ..)` label. The `bucket`
        // resource lives in `types`, and `types`/`atomics`/`cas`/`batch` are bound
        // standalone: a guest imports them unlabeled, and their methods operate on
        // a `bucket` whose `KvBucket` already carries the backend it was opened
        // through, so they route via the resource rather than a label.
        if has_labeled {
            bindings::named_imports::wasmcloud::keyvalue::store::add_to_linker::<_, SharedCtx>(
                linker,
                &component,
                |name| self.mux.resolve(&registry, name),
                extract_active_ctx,
            )?;
        }
        // A plain (unlabeled) `store` import: bind the standard interface to the
        // workload's default backend — no label required.
        if has_plain {
            bindings::wasmcloud::keyvalue::store::add_to_linker::<_, SharedCtx>(
                linker,
                extract_active_ctx,
            )?;
        }
        bindings::wasmcloud::keyvalue::types::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        bindings::wasmcloud::keyvalue::atomics::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        bindings::wasmcloud::keyvalue::cas::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        bindings::wasmcloud::keyvalue::batch::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::wasi_keyvalue::InMemoryProvider;

    fn kv_iface(name: &str) -> WitInterface {
        WitInterface {
            namespace: "wasmcloud".to_string(),
            package: "keyvalue".to_string(),
            interfaces: ["store".to_string()].into_iter().collect(),
            version: None,
            config: std::collections::HashMap::from([(
                "backend".to_string(),
                "in-memory".to_string(),
            )]),
            name: Some(name.to_string()),
        }
    }

    #[test]
    fn store_error_maps_to_async_variant() {
        assert!(matches!(
            AsyncKvError::from(StoreError::NoSuchStore),
            AsyncKvError::NoSuchStore
        ));
        assert!(matches!(
            AsyncKvError::from(StoreError::AccessDenied),
            AsyncKvError::AccessDenied
        ));
        assert!(matches!(
            AsyncKvError::from(StoreError::Other("x".into())),
            AsyncKvError::Other(_)
        ));
    }

    /// The async keyvalue plugin routes two named `wasmcloud:keyvalue` interfaces
    /// to distinct backends — proving the `("wasmcloud", "keyvalue")` multiplexer
    /// is wired and isolation holds, exercising the shared `KvBackend`.
    #[tokio::test]
    async fn registry_routes_named_interfaces_to_distinct_backends() {
        let plugin = MultiplexedAsyncKeyValue::new().with_provider(Arc::new(InMemoryProvider));
        let interfaces = HashSet::from([kv_iface("team-a"), kv_iface("team-b")]);

        let registry = plugin.build_registry(&interfaces).await.unwrap();
        let a = registry.get("team-a").expect("team-a routed").clone();
        let b = registry.get("team-b").expect("team-b routed").clone();

        a.open("bucket").await.unwrap();
        a.set("bucket", "k", b"v".to_vec()).await.unwrap();
        b.open("bucket").await.unwrap();
        assert_eq!(b.get("bucket", "k").await.unwrap(), None, "backends leaked");
        assert_eq!(a.get("bucket", "k").await.unwrap(), Some(b"v".to_vec()));
    }

    #[tokio::test]
    async fn build_registry_errors_on_unregistered_backend() {
        let plugin = MultiplexedAsyncKeyValue::new(); // no providers
        let mut iface = kv_iface("x");
        iface
            .config
            .insert("backend".to_string(), "redis".to_string());
        let err = plugin
            .build_registry(&HashSet::from([iface]))
            .await
            .err()
            .expect("expected error for unregistered backend");
        assert!(err.to_string().contains("redis"), "unexpected error: {err}");
    }
}
