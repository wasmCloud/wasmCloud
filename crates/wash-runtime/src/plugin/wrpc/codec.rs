//! wRPC transport integration types for `WrpcView` on `SharedCtx`.
//!
//! Provides a `RoutingInvoker` that implements `wrpc_transport::Invoke` by
//! delegating to the appropriate `wrpc_transport_nats::Client` based on the
//! WIT instance name. This allows `wrpc_runtime_wasmtime::link_instance` to
//! handle all encoding/decoding automatically.

use std::collections::HashMap;
use std::sync::Arc;

/// Routes wRPC invocations to the correct NATS client based on WIT instance name.
///
/// During binding, import interfaces with `wrpc:name` config are mapped to
/// `wrpc_transport_nats::Client` instances. When `link_instance` calls
/// `store.data_mut().wrpc().ctx.client().invoke(cx, instance, func, ...)`,
/// the RoutingInvoker looks up the instance name and delegates to the
/// corresponding NATS client.
#[derive(Clone, Default)]
pub struct RoutingInvoker {
    /// Map from WIT instance name (e.g. "ns:pkg/iface@0.1.0") to the NATS client
    /// that handles that interface.
    routes: HashMap<Arc<str>, Arc<wrpc_transport_nats::Client>>,
}

impl RoutingInvoker {
    /// Create a new empty routing invoker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a route from a WIT instance name to a NATS client.
    pub fn add_route(
        &mut self,
        instance_name: impl Into<Arc<str>>,
        client: Arc<wrpc_transport_nats::Client>,
    ) {
        self.routes.insert(instance_name.into(), client);
    }
}

impl wrpc_transport::Invoke for RoutingInvoker {
    type Context = Option<async_nats::HeaderMap>;
    type Outgoing = wrpc_transport_nats::ParamWriter;
    type Incoming = wrpc_transport_nats::Reader;

    async fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: bytes::Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let client = self.routes.get(instance).ok_or_else(|| {
            anyhow::anyhow!("no wrpc route configured for instance '{instance}'")
        })?;
        client.invoke(cx, instance, func, params, paths).await
    }
}

/// wRPC context that provides the RoutingInvoker to `link_instance`.
pub struct WrpcState {
    invoker: RoutingInvoker,
    shared_resources: wrpc_runtime_wasmtime::SharedResourceTable,
}

impl WrpcState {
    pub fn new(invoker: RoutingInvoker) -> Self {
        Self {
            invoker,
            shared_resources: Default::default(),
        }
    }
}

impl Default for WrpcState {
    fn default() -> Self {
        Self::new(RoutingInvoker::new())
    }
}

impl wrpc_runtime_wasmtime::WrpcCtx<RoutingInvoker> for WrpcState {
    fn context(&self) -> <RoutingInvoker as wrpc_transport::Invoke>::Context {
        None // No custom NATS headers
    }

    fn client(&self) -> &RoutingInvoker {
        &self.invoker
    }

    fn shared_resources(&mut self) -> &mut wrpc_runtime_wasmtime::SharedResourceTable {
        &mut self.shared_resources
    }
}

