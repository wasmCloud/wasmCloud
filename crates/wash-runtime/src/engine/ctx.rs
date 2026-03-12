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

use wasmtime::component::ResourceTable;
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::plugin::HostPlugin;

/// Shared context for linked components
pub struct SharedCtx {
    /// Current active context
    pub active_ctx: Ctx,
    /// The resource table used to manage resources in the Wasmtime store.
    pub table: wasmtime::component::ResourceTable,
    /// Contexts for linked components
    pub contexts: HashMap<Arc<str>, Ctx>,
}

impl SharedCtx {
    pub fn new(context: Ctx) -> Self {
        Self {
            active_ctx: context,
            table: ResourceTable::new(),
            contexts: Default::default(),
        }
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
    /// Unique identifier for this component context. This is a [uuid::Uuid::new_v4] string.
    pub id: String,
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
    /// Plugin instances stored by string ID for access during component execution.
    /// These all implement the [`HostPlugin`] trait, but they are cast as `Arc<dyn Any + Send + Sync>`
    /// to support downcasting to the specific plugin type in [`Ctx::get_plugin`]
    plugins: HashMap<&'static str, Arc<dyn Any + Send + Sync>>,
    /// The HTTP handler for outgoing HTTP requests.
    http_handler: Option<Arc<dyn crate::host::http::HostHandler>>,
    /// A list of allowed hosts for outgoing HTTP requests.
    allowed_hosts: Arc<[String]>,
}

impl Ctx {
    /// Get a plugin by its string ID and downcast to the expected type
    pub fn get_plugin<T: HostPlugin + 'static>(&self, plugin_id: &str) -> Option<Arc<T>> {
        self.plugins.get(plugin_id)?.clone().downcast().ok()
    }

    /// Create a new [`CtxBuilder`] to construct a [`Ctx`]
    pub fn builder(
        workload_id: impl Into<Arc<str>>,
        component_id: impl Into<Arc<str>>,
    ) -> CtxBuilder {
        CtxBuilder::new(workload_id, component_id)
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

// Implement WasiHttpView for wasi:http@0.2
impl WasiHttpView for SharedCtx {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.active_ctx.http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn send_request(
        &mut self,
        request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::HttpResult<wasmtime_wasi_http::types::HostFutureIncomingResponse> {
        match &self.active_ctx.http_handler {
            Some(handler) => handler.outgoing_request(
                &self.active_ctx.workload_id,
                request,
                config,
                &self.active_ctx.allowed_hosts,
            ),
            None => Err(wasmtime_wasi_http::HttpError::trap(wasmtime::format_err!(
                "http client not available"
            ))),
        }
    }
}

/// Helper struct to build a [`Ctx`] with a builder pattern
pub struct CtxBuilder {
    id: String,
    workload_id: Arc<str>,
    component_id: Arc<str>,
    ctx: Option<WasiCtx>,
    sockets: Option<crate::sockets::WasiSocketsCtx>,
    plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
    http_handler: Option<Arc<dyn crate::host::http::HostHandler>>,
    allowed_hosts: Arc<[String]>,
}

impl CtxBuilder {
    pub fn new(workload_id: impl Into<Arc<str>>, component_id: impl Into<Arc<str>>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            component_id: component_id.into(),
            workload_id: workload_id.into(),
            ctx: None,
            sockets: None,
            http_handler: None,
            plugins: HashMap::new(),
            allowed_hosts: Default::default(),
        }
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

    pub fn with_http_handler(
        mut self,
        http_handler: Arc<dyn crate::host::http::HostHandler>,
    ) -> Self {
        self.http_handler = Some(http_handler);
        self
    }

    pub fn with_plugins(
        mut self,
        plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
    ) -> Self {
        self.plugins.extend(plugins);
        self
    }

    pub fn with_allowed_hosts(mut self, allowed_hosts: Arc<[String]>) -> Self {
        self.allowed_hosts = allowed_hosts;
        self
    }

    pub fn build(self) -> Ctx {
        let plugins = self
            .plugins
            .into_iter()
            .map(|(k, v)| (k, v as Arc<dyn Any + Send + Sync>))
            .collect();

        Ctx {
            id: self.id,
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
            plugins,
            http_handler: self.http_handler,
            allowed_hosts: self.allowed_hosts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctx_builder_sets_ids() {
        let ctx = Ctx::builder("wk-1", "comp-1").build();
        assert_eq!(ctx.workload_id.as_ref(), "wk-1");
        assert_eq!(ctx.component_id.as_ref(), "comp-1");
    }

    #[test]
    fn ctx_builder_generates_uuid_id() {
        let ctx = Ctx::builder("wk", "comp").build();
        // id should be a valid UUID v4 string
        assert!(uuid::Uuid::parse_str(&ctx.id).is_ok());
    }

    #[test]
    fn ctx_builder_uses_default_wasi_ctx_when_none_provided() {
        // Should not panic — proves default WasiCtx is created
        let _ctx = Ctx::builder("wk", "comp").build();
    }

    #[test]
    fn shared_ctx_new_sets_active_ctx() {
        let ctx = Ctx::builder("wk", "comp-a").build();
        let shared = SharedCtx::new(ctx);
        assert_eq!(shared.active_ctx.component_id.as_ref(), "comp-a");
        assert!(shared.contexts.is_empty());
    }

    #[test]
    fn set_active_ctx_swaps_context() {
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
        let ctx = Ctx::builder("wk", "comp-a").build();
        let mut shared = SharedCtx::new(ctx);
        let unknown: Arc<str> = Arc::from("nonexistent");
        let result = shared.set_active_ctx(&unknown);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn set_active_ctx_is_noop_when_already_active() {
        let ctx = Ctx::builder("wk", "comp-a").build();
        let mut shared = SharedCtx::new(ctx);
        let comp_a: Arc<str> = Arc::from("comp-a");
        // Should succeed and be a no-op
        shared.set_active_ctx(&comp_a).unwrap();
        assert_eq!(shared.active_ctx.component_id.as_ref(), "comp-a");
        assert!(shared.contexts.is_empty());
    }
}
