use super::{Ctx, Instance, InterfaceInstance};

use crate::capability::http::types;
use crate::capability::{IncomingHttp, OutgoingHttp};

use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use tokio::sync::{oneshot, Mutex};
use wasmtime::component::{Resource, ResourceTable};
use wasmtime_wasi::preview2::{self};
use wasmtime_wasi_http::body::{HyperIncomingBody, HyperOutgoingBody};
use wasmtime_wasi_http::types::{
    HostFutureIncomingResponse, IncomingResponseInternal, OutgoingRequest,
};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

pub mod incoming_http_bindings {
    wasmtime::component::bindgen!({
        world: "incoming-http",
        async: true,
        with: {
           "wasi:http/types": wasmtime_wasi_http::bindings::http::types,
        },
    });
}

impl WasiHttpView for Ctx {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn send_request(
        &mut self,
        request: OutgoingRequest,
    ) -> wasmtime::Result<Resource<HostFutureIncomingResponse>>
    where
        Self: Sized,
    {
        let handler = self.handler.clone();
        let between_bytes_timeout = request.between_bytes_timeout;
        let res = HostFutureIncomingResponse::new(preview2::spawn(async move {
            match OutgoingHttp::handle(&handler, request).await {
                Ok(Ok(resp)) => Ok(Ok(IncomingResponseInternal {
                    resp,
                    worker: Arc::new(preview2::spawn(async {})),
                    between_bytes_timeout,
                })),
                Ok(Err(err)) => Ok(Err(err)),
                Err(e) => Err(e),
            }
        }));
        let res = self.table().push(res).context("failed to push response")?;
        Ok(res)
    }
}

impl Instance {
    /// Set [`IncomingHttp`] handler for this [Instance].
    pub fn incoming_http(
        &mut self,
        incoming_http: Arc<dyn IncomingHttp + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut().replace_incoming_http(incoming_http);
        self
    }

    /// Set [`OutgoingHttp`] handler for this [Instance].
    pub fn outgoing_http(
        &mut self,
        outgoing_http: Arc<dyn OutgoingHttp + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut().replace_outgoing_http(outgoing_http);
        self
    }

    /// Instantiates and returns a [`InterfaceInstance<incoming_http_bindings::IncomingHttp>`] if exported by the [`Instance`].
    ///
    /// # Errors
    ///
    /// Fails if incoming HTTP bindings are not exported by the [`Instance`]
    pub async fn into_incoming_http(
        mut self,
    ) -> anyhow::Result<InterfaceInstance<incoming_http_bindings::IncomingHttp>> {
        let (bindings, _) = incoming_http_bindings::IncomingHttp::instantiate_async(
            &mut self.store,
            &self.component,
            &self.linker,
        )
        .await?;
        Ok(InterfaceInstance {
            store: Mutex::new(self.store),
            bindings,
        })
    }
}

#[async_trait]
impl IncomingHttp for InterfaceInstance<incoming_http_bindings::IncomingHttp> {
    async fn handle(
        &self,
        request: http::Request<HyperIncomingBody>,
    ) -> anyhow::Result<Result<http::Response<HyperOutgoingBody>, types::ErrorCode>> {
        let mut store = self.store.lock().await;
        let ctx = store.data_mut();
        let request = ctx
            .new_incoming_request(request)
            .context("failed to create incoming request")?;
        let (response_tx, mut response_rx) = oneshot::channel();
        let response = ctx
            .new_response_outparam(response_tx)
            .context("failed to create response")?;
        self.bindings
            .wasi_http_incoming_handler()
            .call_handle(&mut *store, request, response)
            .await?;
        response_rx.try_recv().context("a response was not set")
    }
}
