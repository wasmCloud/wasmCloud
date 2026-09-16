//! Component execution context for wasmtime stores.
//!
//! This module provides the [`Ctx`] type which serves as the store context
//! for wasmtime when executing WebAssembly components. It integrates WASI
//! interfaces, HTTP capabilities, and plugin access into a unified context.

use std::{
    any::Any,
    collections::HashMap,
    ops::{Deref, DerefMut},
    sync::Arc,
};

use wasmtime::StoreContextMut;
use wasmtime::component::{Accessor, Instance, ResourceTable};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpHooks, WasiHttpView};
#[cfg(feature = "wasi-tls")]
use wasmtime_wasi_tls::{WasiTlsCtx, WasiTlsCtxBuilder, WasiTlsCtxView, WasiTlsView};

use crate::host::allowed_hosts::AllowedHost;
use crate::plugin::HostPlugin;

/// A shareable, cloneable `wasi:tls` provider. Wraps an `Arc<dyn TlsProvider>`
/// so the same provider can back many per-component contexts without
/// re-creating it, and implements `TlsProvider` directly so it can be boxed
/// and passed to `WasiTlsCtxBuilder::provider`.
#[cfg(feature = "wasi-tls")]
#[derive(Clone)]
pub struct SharedTlsProvider(Arc<dyn wasmtime_wasi_tls::TlsProvider>);

#[cfg(feature = "wasi-tls")]
impl SharedTlsProvider {
    pub fn new(provider: impl wasmtime_wasi_tls::TlsProvider + 'static) -> Self {
        Self(Arc::new(provider))
    }
}

/// Concrete return type of [`wasmtime_wasi_tls::TlsProvider::connect`]. The
/// upstream alias (`BoxFutureTlsStream`) is `pub(crate)`, so we re-declare an
/// equivalent here to keep the impl signature readable.
#[cfg(feature = "wasi-tls")]
type TlsConnectFuture = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<Box<dyn wasmtime_wasi_tls::TlsStream>, wasmtime_wasi_tls::Error>,
            > + Send,
    >,
>;

#[cfg(feature = "wasi-tls")]
impl wasmtime_wasi_tls::TlsProvider for SharedTlsProvider {
    fn connect(
        &self,
        server_name: String,
        transport: Box<dyn wasmtime_wasi_tls::TlsTransport>,
    ) -> TlsConnectFuture {
        self.0.connect(server_name, transport)
    }
}

/// Shared context for linked components
pub struct SharedCtx {
    /// Current active context
    pub active_ctx: Ctx,
    /// The resource table used to manage resources in the Wasmtime store.
    pub table: wasmtime::component::ResourceTable,
    /// Contexts for linked components
    pub contexts: HashMap<Arc<str>, Ctx>,
    /// Store-owned linked-component instances, keyed by component id.
    /// Dropping the store reclaims the instances without external cleanup.
    pub exporter_instances: HashMap<Arc<str>, Instance>,
    /// Present only on a *host component plugin* store: the registry of real
    /// resources it has handed out across the bridge. Its presence also marks
    /// this store as the plugin (real) side when relocating `resource` handles —
    /// a caller store leaves it `None` and holds opaque proxies instead. See
    /// [`crate::engine::store::resource_bridge`].
    pub resource_registry: Option<crate::engine::store::resource_bridge::ResourceRegistry>,
    /// The in-flight calls on this store whose dispatchers may abandon them;
    /// read by the store's epoch callback. See [`crate::engine::abandon`].
    pub abandoned: Arc<crate::engine::abandon::AbandonedCalls>,
    /// Guest execution on this store, sampled by the same epoch callback and
    /// reported by [`crate::observability::ExecutionTimeMeter`]. Read its docs
    /// before reading the number: it is a floor, not a total.
    pub executed: Arc<crate::engine::abandon::GuestExecution>,
    /// This store's share of the host's guest memory budget, installed on the
    /// store by [`crate::engine::guest_memory::install_memory_limiter`]. Living
    /// here is what makes the release exact: dropping the store drops this and
    /// hands the bytes back.
    pub memory_limiter: crate::engine::guest_memory::StoreMemoryLimiter,
    /// Notifies waiters after all other store data is dropped.
    ///
    /// Wasmtime disposes guest fibers and host tasks before dropping store data.
    pub dropped: StoreDropped,
}

/// Lazily creates a token and cancels it when dropped.
#[derive(Default)]
pub struct StoreDropped(std::sync::OnceLock<tokio_util::sync::CancellationToken>);

impl StoreDropped {
    /// Returns a token canceled when this value drops.
    pub fn token(&self) -> tokio_util::sync::CancellationToken {
        self.0
            .get_or_init(tokio_util::sync::CancellationToken::new)
            .clone()
    }
}

impl Drop for StoreDropped {
    fn drop(&mut self) {
        if let Some(token) = self.0.get() {
            token.cancel();
        }
    }
}

/// The identity of whoever is invoking a host component plugin, used to
/// partition state per caller.
///
/// `component_id` is `None` when there is no component behind the call: a
/// `wasmcloud:host/workload-lifecycle` hook is delivered by the host about a
/// whole workload, not on behalf of any one of its items. Encoding that as a
/// real id — an empty string, say — would make the host invent a component that
/// does not exist, and would collide with a genuinely unresolvable caller.
#[derive(Clone, Debug)]
pub struct CallerIdentity {
    pub workload_id: Arc<str>,
    pub component_id: Option<Arc<str>>,
    /// The binding the call arrived on: the `(implements ..)` label the caller
    /// imported the plugin's interface under, which is also the name the
    /// operator declared it by. `None` for a plain import, and for a lifecycle
    /// hook, which is about a whole workload rather than one binding.
    ///
    /// Read by `wasmcloud:host/identity#get-binding-name`, so a plugin serving
    /// two bindings of one workload can pair a call with the configuration that
    /// binding was resolved with.
    pub binding: Option<Arc<str>>,
}

impl SharedCtx {
    pub fn new(context: Ctx) -> Self {
        Self {
            active_ctx: context,
            table: ResourceTable::new(),
            contexts: Default::default(),
            exporter_instances: Default::default(),
            resource_registry: None,
            abandoned: Arc::default(),
            executed: Arc::default(),
            memory_limiter: Default::default(),
            dropped: StoreDropped::default(),
        }
    }

    /// Marks this store as a host-component-plugin store, enabling it to keep
    /// real resources alive as it hands proxies across the bridge.
    pub fn with_resource_registry(mut self) -> Self {
        self.resource_registry = Some(Default::default());
        self
    }

    /// Draws this store's linear memory from `budget`. Without this the store
    /// is neither charged nor limited.
    pub fn with_guest_memory(
        mut self,
        budget: &Arc<crate::engine::guest_memory::GuestMemoryBudget>,
    ) -> Self {
        self.memory_limiter = budget.limiter();
        self
    }

    pub fn set_active_ctx(&mut self, id: &Arc<str>) -> wasmtime::Result<()> {
        if id == &self.active_ctx.component_id {
            return Ok(());
        }

        if let Some(ctx) = self.contexts.remove(id) {
            let old_ctx = std::mem::replace(&mut self.active_ctx, ctx);
            self.contexts.insert(old_ctx.component_id.clone(), old_ctx);
            Ok(())
        } else {
            Err(wasmtime::format_err!(
                "Context for component {id} not found"
            ))
        }
    }
}

/// RAII guard that points [`SharedCtx::active_ctx`] at a linked component for
/// the duration of a cross-component call, restoring the previous component on
/// drop.
///
/// TODO(concurrency): the single `active_ctx` slot with LIFO save/restore is
/// only correct for *sequential* linked calls. `func_new_concurrent` allows
/// several in flight on one shared store, and their save/restore interleaves —
/// A sets B; C (concurrent) saves B, sets D; A restores B; C restores B — so
/// the original ctx is lost and `active_ctx` is wrong even after both finish
/// (and mid-flight, across awaits, it need not match the running call). The
/// single-call fixtures issue one at a time so they don't exercise this; a real
/// fix needs per-call ctx scoping rather than a shared slot. Tracked follow-up.
pub(crate) struct AccessorActiveCtxGuard<'a> {
    accessor: &'a Accessor<SharedCtx>,
    previous_component_id: Arc<str>,
}

impl<'a> AccessorActiveCtxGuard<'a> {
    pub(crate) fn new(accessor: &'a Accessor<SharedCtx>, id: &Arc<str>) -> wasmtime::Result<Self> {
        let previous_component_id = accessor.with(|mut access| -> wasmtime::Result<_> {
            let previous_component_id = access.data_mut().active_ctx.component_id.clone();
            access.data_mut().set_active_ctx(id)?;
            Ok(previous_component_id)
        })?;

        Ok(Self {
            accessor,
            previous_component_id,
        })
    }
}

impl Drop for AccessorActiveCtxGuard<'_> {
    fn drop(&mut self) {
        let _ = self.accessor.with(|mut access| {
            access
                .data_mut()
                .set_active_ctx(&self.previous_component_id)
        });
    }
}

pub(crate) struct StoreActiveCtxGuard<'a> {
    store: StoreContextMut<'a, SharedCtx>,
    previous_component_id: Arc<str>,
}

impl<'a> StoreActiveCtxGuard<'a> {
    pub(crate) fn new(
        mut store: StoreContextMut<'a, SharedCtx>,
        id: &Arc<str>,
    ) -> wasmtime::Result<Self> {
        let previous_component_id = store.data().active_ctx.component_id.clone();
        store.data_mut().set_active_ctx(id)?;
        Ok(Self {
            store,
            previous_component_id,
        })
    }

    pub(crate) fn store_mut(&mut self) -> &mut StoreContextMut<'a, SharedCtx> {
        &mut self.store
    }
}

impl Drop for StoreActiveCtxGuard<'_> {
    fn drop(&mut self) {
        let _ = self
            .store
            .data_mut()
            .set_active_ctx(&self.previous_component_id);
    }
}

impl wasmtime::component::HasData for SharedCtx {
    type Data<'a> = ActiveCtx<'a>;
}

pub fn extract_active_ctx(ctx: &mut SharedCtx) -> ActiveCtx<'_> {
    ActiveCtx {
        table: &mut ctx.table,
        ctx: &mut ctx.active_ctx,
    }
}

pub fn extract_sockets(ctx: &mut SharedCtx) -> crate::sockets::WasiSocketsCtxView<'_> {
    crate::sockets::WasiSocketsCtxView {
        ctx: &mut ctx.active_ctx.sockets,
        table: &mut ctx.table,
    }
}

pub struct ActiveCtx<'a> {
    pub table: &'a mut wasmtime::component::ResourceTable,
    pub ctx: &'a mut Ctx,
}

impl<'a> Deref for ActiveCtx<'a> {
    type Target = Ctx;

    fn deref(&self) -> &Self::Target {
        self.ctx
    }
}

impl<'a> DerefMut for ActiveCtx<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ctx
    }
}

/// The context for a component store and linker, providing access to implementations of:
/// - wasi@0.2 interfaces
/// - wasi:http@0.2 interfaces
pub struct Ctx {
    /// Unique identifier for this component context. This is a [uuid::Uuid::now_v7] string.
    pub id: Arc<str>,
    /// Unique identifier shared by all component contexts in the same store.
    pub store_id: Arc<str>,
    /// The unique identifier for the workload component this instance belongs to
    pub component_id: Arc<str>,
    /// The unique identifier for the workload this component belongs to
    pub workload_id: Arc<str>,
    /// The WASI context used to provide WASI functionality to the components using this context.
    pub ctx: WasiCtx,
    /// The HTTP context used to provide HTTP functionality to the component.
    pub http: WasiHttpCtx,
    /// The sockets context used to provide socket functionality (with loopback support).
    pub sockets: crate::sockets::WasiSocketsCtx,
    /// The TLS Context used to provide TLS over the HTTP functionality to the component.
    #[cfg(feature = "wasi-tls")]
    pub tls: WasiTlsCtx,
    /// Plugin instances stored by string ID for access during component execution.
    /// These all implement the [`HostPlugin`] trait, but they are cast as `Arc<dyn Any + Send + Sync>`
    /// to support downcasting to the specific plugin type in [`Ctx::get_plugin`]
    plugins: HashMap<&'static str, Arc<dyn Any + Send + Sync>>,
    /// The HTTP hooks for outgoing HTTP requests.
    http_hooks: CtxHttpHooks,
}

impl Ctx {
    /// Get a plugin by its string ID and downcast to the expected type
    ///
    /// **Panics** if the plugin is not found or does not match the expected type.
    #[allow(clippy::expect_used)] // Infallible accessor by contract; callers needing fallibility use `try_get_plugin`
    pub fn get_plugin<T: HostPlugin + 'static>(&self, plugin_id: &str) -> Arc<T> {
        self.try_get_plugin::<T>(plugin_id)
            .expect("plugin not found")
    }

    /// Get a reference to a plugin by its string ID and downcast to the expected type
    ///
    /// Unlike [`Ctx::get_plugin`], this borrows the plugin from `self` instead of
    /// cloning the `Arc`, allowing callers to return references tied to `&self`.
    ///
    /// **Panics** if the plugin is not found or does not match the expected type.
    #[allow(clippy::expect_used)] // Infallible accessor by contract, mirrors `get_plugin`
    pub fn get_plugin_ref<T: HostPlugin + 'static>(&self, plugin_id: &str) -> &T {
        self.plugins
            .get(plugin_id)
            .expect("plugin not found")
            .downcast_ref::<T>()
            .expect("failed to downcast plugin to expected type")
    }

    /// Get a plugin by its string ID and downcast to the expected type, if it exists
    pub fn try_get_plugin<T: HostPlugin + 'static>(
        &self,
        plugin_id: &str,
    ) -> wasmtime::Result<Arc<T>> {
        self.plugins
            .get(plugin_id)
            .ok_or_else(|| wasmtime::format_err!("plugin {plugin_id} not found"))?
            .clone()
            .downcast::<T>()
            .map_err(|_| {
                wasmtime::format_err!(
                    "failed to downcast plugin to type {}",
                    std::any::type_name::<T>()
                )
            })
    }

    /// Create a new [`CtxBuilder`] to construct a [`Ctx`]
    pub fn builder(
        workload_id: impl Into<Arc<str>>,
        component_id: impl Into<Arc<str>>,
    ) -> CtxBuilder {
        CtxBuilder::new(workload_id, component_id)
    }

    /// This store's link back to the host that built it, for readers that need
    /// to know whether that host is still there; see [`HostLink`].
    pub(crate) fn host_link(&self) -> HostLink {
        self.http_hooks.host.clone()
    }
}

impl std::fmt::Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx")
            .field("id", &self.id)
            .field("workload_id", &self.workload_id.as_ref())
            .finish()
    }
}

// TODO(#103): Do some cleverness to pull up the WasiCtx based on what component is actively executing
impl WasiView for SharedCtx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.active_ctx.ctx,
            table: &mut self.table,
        }
    }
}

impl wasmtime_wasi_io::IoView for SharedCtx {
    fn table(&mut self) -> &mut wasmtime_wasi::ResourceTable {
        &mut self.table
    }
}

impl WasiHttpView for SharedCtx {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.active_ctx.http,
            table: &mut self.table,
            hooks: &mut self.active_ctx.http_hooks,
        }
    }
}

#[cfg(feature = "wasi-tls")]
impl WasiTlsView for SharedCtx {
    fn tls(&mut self) -> WasiTlsCtxView<'_> {
        WasiTlsCtxView {
            ctx: &mut self.active_ctx.tls,
            table: &mut self.table,
        }
    }
}

/// A store's link back to the host that built it, held weak; see
/// [`CtxBuilder::with_host`].
#[derive(Clone, Default)]
pub(crate) struct HostLink(Option<crate::host::HostRef>);

impl HostLink {
    /// The handler to serve this store's egress with, or why there is none. A
    /// store with no host at all and a store whose host has gone away under it
    /// are different failures, and the guest is told which.
    fn handler(&self) -> Result<Arc<dyn crate::host::http::HostHandler>, String> {
        match &self.0 {
            None => Err("http client not available".to_string()),
            Some(host) if !host.has_handler() => Err("http client not available".to_string()),
            Some(host) => crate::host::http::live_handler(host).map_err(|e| format!("{e:#}")),
        }
    }

    /// Whether the host that built this store is gone, asked of the host's own
    /// lifetime rather than of its handler: an embedder may hold a clone of the
    /// handler long after the host is dropped, and a store that outlived its
    /// host has to be ended either way. A store built without a host answers
    /// `false` — never pointed at one, it can say nothing about one.
    pub(crate) fn host_is_gone(&self) -> bool {
        self.0.as_ref().is_some_and(crate::host::HostRef::host_is_gone)
    }
}

/// HTTP hooks that delegate outgoing requests to the configured
/// [`HostHandler`](crate::host::http::HostHandler), so custom egress
/// (allowed-hosts policy, alternate transports, etc.) applies to `wasi:http`
/// 0.2 and 0.3 components alike.
struct CtxHttpHooks {
    host: HostLink,
    workload_id: Arc<str>,
    allowed_hosts: Arc<[AllowedHost]>,
    /// Set once this store has reported that its egress cannot be served, so a
    /// guest that keeps calling out does not repeat the warning per call.
    warned_unserved: bool,
}

impl WasiHttpHooks for CtxHttpHooks {
    fn send_request(
        &mut self,
        request: hyper::Request<wasmtime_wasi_http::WasiBody>,
        options: Option<wasmtime_wasi_http::RequestOptions>,
        fut: crate::host::http::RequestIoFuture,
    ) -> crate::host::http::SendFuture {
        match self.host.handler() {
            Ok(handler) => handler.outgoing_request(
                &self.workload_id,
                request,
                options,
                fut,
                &self.allowed_hosts,
            ),
            Err(message) => {
                // The hook cannot trap, so the guest gets a handleable error
                // and the host gets told once. A guest left calling out to a
                // host that is gone is ended by the store's epoch deadline
                // instead; see [`crate::engine::abandon::arm_epoch_deadline`].
                if !self.warned_unserved {
                    self.warned_unserved = true;
                    tracing::warn!(
                        workload_id = %self.workload_id,
                        %message,
                        "a component called out over HTTP with no handler to serve it"
                    );
                }
                Box::new(
                    async move { Err(wasmtime_wasi_http::Error::InternalError(Some(message))) },
                )
            }
        }
    }
}

/// Helper struct to build a [`Ctx`] with a builder pattern
pub struct CtxBuilder {
    id: Arc<str>,
    store_id: Arc<str>,
    workload_id: Arc<str>,
    component_id: Arc<str>,
    ctx: Option<WasiCtx>,
    sockets: Option<crate::sockets::WasiSocketsCtx>,
    plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
    http_handler: Option<crate::host::HostRef>,
    allowed_hosts: Arc<[AllowedHost]>,
    /// TLS provider override for `wasi:tls` client connections.
    #[cfg(feature = "wasi-tls")]
    tls_provider: Option<SharedTlsProvider>,
}

impl CtxBuilder {
    pub fn new(workload_id: impl Into<Arc<str>>, component_id: impl Into<Arc<str>>) -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string().into(),
            store_id: uuid::Uuid::now_v7().to_string().into(),
            component_id: component_id.into(),
            workload_id: workload_id.into(),
            ctx: None,
            sockets: None,
            http_handler: None,
            plugins: HashMap::new(),
            allowed_hosts: Default::default(),
            #[cfg(feature = "wasi-tls")]
            tls_provider: None,
        }
    }

    /// Override the TLS provider used for `wasi:tls` client connections.
    ///
    /// Use this to plug in an alternative TLS backend, install a custom root
    /// certificate store (corporate CAs, certificate pinning), or integrate
    /// with HSM-backed key material.
    #[cfg(feature = "wasi-tls")]
    pub fn with_tls_provider(mut self, provider: SharedTlsProvider) -> Self {
        self.tls_provider = Some(provider);
        self
    }

    /// Set a custom [WasiCtx]
    pub fn with_wasi_ctx(mut self, ctx: WasiCtx) -> Self {
        self.ctx = Some(ctx);
        self
    }

    pub fn with_sockets(mut self, sockets: crate::sockets::WasiSocketsCtx) -> Self {
        self.sockets = Some(sockets);
        self
    }

    /// Point this store back at the host that built it: its outgoing HTTP goes
    /// through that host's handler, and it is ended if that host is torn down
    /// while it still runs (see
    /// [`crate::engine::abandon::arm_epoch_deadline`]).
    ///
    /// Weak, and taken weak: building a store must not depend on the host
    /// still being there, because a store is built on paths — a message
    /// delivery, a service restart — that have nothing to do with egress. Only
    /// an actual outbound call needs a live handler, and the hooks report it
    /// per call. See [`crate::host::http::live_handler`].
    pub fn with_host(mut self, host: &crate::host::HostRef) -> Self {
        self.http_handler = Some(host.clone());
        self
    }

    pub fn with_plugins(
        mut self,
        plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
    ) -> Self {
        self.plugins.extend(plugins);
        self
    }

    pub fn with_allowed_hosts(mut self, allowed_hosts: Arc<[AllowedHost]>) -> Self {
        self.allowed_hosts = allowed_hosts;
        self
    }

    pub fn build(self) -> Ctx {
        let plugins = self
            .plugins
            .into_iter()
            .map(|(k, v)| (k, v as Arc<dyn Any + Send + Sync>))
            .collect();

        let http_hooks = CtxHttpHooks {
            host: HostLink(self.http_handler),
            workload_id: self.workload_id.clone(),
            allowed_hosts: self.allowed_hosts,
            warned_unserved: false,
        };

        Ctx {
            id: self.id,
            store_id: self.store_id,
            ctx: self.ctx.unwrap_or_else(|| {
                WasiCtxBuilder::new()
                    .args(&["main.wasm"])
                    .inherit_stderr()
                    .build()
            }),
            workload_id: self.workload_id,
            component_id: self.component_id,
            http: WasiHttpCtx::new(),
            sockets: self.sockets.unwrap_or_default(),
            #[cfg(feature = "wasi-tls")]
            tls: {
                // EngineBuilder::build already warns once if no provider was
                // set; just fall through to the wasmtime-wasi-tls default
                // here when none is provided.
                let mut builder = WasiTlsCtxBuilder::new();
                if let Some(provider) = self.tls_provider {
                    builder = builder.provider(Box::new(provider));
                }
                builder.build()
            },
            plugins,
            http_hooks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() {
        #[cfg(feature = "wasi-tls")]
        crate::init_crypto();
    }

    #[test]
    fn ctx_builder_sets_ids() {
        setup();
        let ctx = Ctx::builder("wk-1", "comp-1").build();
        assert_eq!(ctx.workload_id.as_ref(), "wk-1");
        assert_eq!(ctx.component_id.as_ref(), "comp-1");
    }

    #[test]
    fn ctx_builder_generates_uuid_id() {
        setup();
        let ctx = Ctx::builder("wk", "comp").build();
        // id and store_id should be valid UUID v7 strings
        let id = uuid::Uuid::parse_str(&ctx.id).expect("id is a UUID");
        let store_id = uuid::Uuid::parse_str(&ctx.store_id).expect("store_id is a UUID");
        assert_eq!(id.get_version_num(), 7);
        assert_eq!(store_id.get_version_num(), 7);
    }

    #[test]
    fn ctx_builder_uses_default_wasi_ctx_when_none_provided() {
        setup();
        // Should not panic — proves default WasiCtx is created
        let _ctx = Ctx::builder("wk", "comp").build();
    }

    #[test]
    fn shared_ctx_new_sets_active_ctx() {
        setup();
        let ctx = Ctx::builder("wk", "comp-a").build();
        let shared = SharedCtx::new(ctx);
        assert_eq!(shared.active_ctx.component_id.as_ref(), "comp-a");
        assert!(shared.contexts.is_empty());
    }

    #[test]
    fn set_active_ctx_swaps_context() {
        setup();
        let ctx_a = Ctx::builder("wk", "comp-a").build();
        let ctx_b = Ctx::builder("wk", "comp-b").build();
        let comp_b_id: Arc<str> = Arc::from("comp-b");

        let mut shared = SharedCtx::new(ctx_a);
        shared.contexts.insert(comp_b_id.clone(), ctx_b);

        shared.set_active_ctx(&comp_b_id).unwrap();
        assert_eq!(shared.active_ctx.component_id.as_ref(), "comp-b");
        // The old context should now be in the map
        assert!(
            shared
                .contexts
                .contains_key(&Arc::from("comp-a") as &Arc<str>)
        );
    }

    #[test]
    fn set_active_ctx_returns_error_for_unknown_id() {
        setup();
        let ctx = Ctx::builder("wk", "comp-a").build();
        let mut shared = SharedCtx::new(ctx);
        let unknown: Arc<str> = Arc::from("nonexistent");
        let result = shared.set_active_ctx(&unknown);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn set_active_ctx_is_noop_when_already_active() {
        setup();
        let ctx = Ctx::builder("wk", "comp-a").build();
        let mut shared = SharedCtx::new(ctx);
        let comp_a: Arc<str> = Arc::from("comp-a");
        // Should succeed and be a no-op
        shared.set_active_ctx(&comp_a).unwrap();
        assert_eq!(shared.active_ctx.component_id.as_ref(), "comp-a");
        assert!(shared.contexts.is_empty());
    }

    #[test]
    fn store_active_ctx_guard_restores_on_drop() {
        setup();
        use wasmtime::AsContextMut;

        let mut config = wasmtime::Config::new();
        config.wasm_component_model(true);
        let engine = wasmtime::Engine::new(&config).unwrap();

        let ctx_a = Ctx::builder("wk", "comp-a").build();
        let ctx_b = Ctx::builder("wk", "comp-b").build();
        let comp_b_id: Arc<str> = Arc::from("comp-b");

        let mut shared = SharedCtx::new(ctx_a);
        shared.contexts.insert(comp_b_id.clone(), ctx_b);
        let mut store = wasmtime::Store::new(&engine, shared);

        {
            let mut guard = StoreActiveCtxGuard::new(store.as_context_mut(), &comp_b_id).unwrap();
            assert_eq!(
                guard.store_mut().data().active_ctx.component_id.as_ref(),
                "comp-b"
            );
        }

        assert_eq!(store.data().active_ctx.component_id.as_ref(), "comp-a");
    }

    /// A store an embedder built for itself must never be ended by
    /// [`crate::engine::abandon::arm_epoch_deadline`]'s host check: it was
    /// never pointed at a host, so it can say nothing about one. The host side
    /// of this contract is
    /// `a_store_sees_its_host_go_away_though_a_handler_clone_remains`.
    #[test]
    fn a_store_built_without_a_host_reports_none_gone() {
        setup();
        assert!(
            !Ctx::builder("wk", "comp").build().host_link().host_is_gone(),
            "a store with no host must not report one gone"
        );
    }
}
