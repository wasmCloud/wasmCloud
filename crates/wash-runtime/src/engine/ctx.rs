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
    /// wRPC context providing the routing invoker for `link_instance` import polyfilling,
    /// and resource table access during encode/decode for export serving.
    #[cfg(feature = "wrpc")]
    pub(crate) wrpc_ctx: crate::plugin::wrpc::codec::WrpcState,
}

impl SharedCtx {
    pub fn new(context: Ctx) -> Self {
        Self {
            active_ctx: context,
            table: ResourceTable::new(),
            contexts: Default::default(),
            #[cfg(feature = "wrpc")]
            wrpc_ctx: Default::default(),
        }
    }

    pub fn set_active_ctx(&mut self, id: &Arc<str>) -> anyhow::Result<()> {
        if id == &self.active_ctx.component_id {
            return Ok(());
        }

        if let Some(ctx) = self.contexts.remove(id) {
            let old_ctx = std::mem::replace(&mut self.active_ctx, ctx);
            self.contexts.insert(old_ctx.component_id.clone(), old_ctx);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Context for component {id} not found"))
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
            Some(handler) => {
                handler.outgoing_request(&self.active_ctx.workload_id, request, config)
            }
            None => Err(wasmtime_wasi_http::HttpError::trap(anyhow::anyhow!(
                "http client not available"
            ))),
        }
    }
}

// Implement WrpcView for wrpc-runtime-wasmtime codec support.
// The NoopInvoker is never actually used for transport — real wRPC invocations
// go through captured `wrpc_transport_nats::Client` instances in plugin closures.
#[cfg(feature = "wrpc")]
impl wrpc_runtime_wasmtime::WrpcView for SharedCtx {
    type Invoke = crate::plugin::wrpc::codec::RoutingInvoker;

    fn wrpc(
        &mut self,
    ) -> wrpc_runtime_wasmtime::WrpcCtxView<'_, crate::plugin::wrpc::codec::RoutingInvoker> {
        wrpc_runtime_wasmtime::WrpcCtxView {
            ctx: &mut self.wrpc_ctx,
            table: &mut self.table,
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
        }
    }
}
