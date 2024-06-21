use super::{Ctx, Instance, InterfaceInstance};

use crate::capability::http::types;
use crate::capability::{IncomingHttp, OutgoingHttp};

use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use tokio::sync::{oneshot, Mutex};
use tracing::instrument;
use wasmtime::component::ResourceTable;
use wasmtime_wasi_http::body::{HyperIncomingBody, HyperOutgoingBody};
use wasmtime_wasi_http::types::{
    HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig,
};
use wasmtime_wasi_http::{HttpResult, WasiHttpCtx, WasiHttpView};

pub mod incoming_http_bindings {
    wasmtime::component::bindgen!({
        world: "incoming-http",
        async: true,
        trappable_imports: true,
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
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse>
    where
        Self: Sized,
    {
        let handler = self.handler.clone();
        let between_bytes_timeout = config.between_bytes_timeout.clone();
        let handle = wasmtime_wasi::runtime::spawn(async move {
            match OutgoingHttp::handle(&handler, request, config).await {
                Ok(Ok(resp)) => Ok(Ok(IncomingResponse {
                    resp,
                    worker: Some(wasmtime_wasi::runtime::spawn(async {})),
                    between_bytes_timeout,
                })),
                Ok(Err(err)) => Ok(Err(err)),
                Err(e) => Err(e),
            }
        });

        Ok(HostFutureIncomingResponse::Pending(handle))
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
        let (bindings, _) = incoming_http_bindings::IncomingHttp::instantiate_pre(
            &mut self.store,
            &self.instance_pre,
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
    #[instrument(skip_all)]
    async fn handle(
        &self,
        request: http::Request<HyperIncomingBody>,
        response: oneshot::Sender<Result<http::Response<HyperOutgoingBody>, types::ErrorCode>>,
    ) -> anyhow::Result<()> {
        let mut store = self.store.lock().await;
        let ctx = store.data_mut();
        let request = ctx
            .new_incoming_request(request)
            .context("failed to create incoming request")?;
        let response = ctx
            .new_response_outparam(response)
            .context("failed to create response")?;
        self.bindings
            .wasi_http_incoming_handler()
            .call_handle(&mut *store, request, response)
            .await
    }
}
