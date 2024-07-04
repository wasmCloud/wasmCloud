use super::{new_store, Ctx, Handler, Instance, WrpcServeEvent};

use crate::capability::http::types;

use anyhow::{bail, Context as _};
use futures::stream::StreamExt as _;
use tokio::sync::oneshot;
use tokio::{join, spawn};
use tracing::{instrument, warn};
use wasmtime::component::ResourceTable;
use wasmtime_wasi_http::body::HyperOutgoingBody;
use wasmtime_wasi_http::types::{
    HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig,
};
use wasmtime_wasi_http::{HttpResult, WasiHttpCtx, WasiHttpView};
use wrpc_interface_http::ServeIncomingHandlerWasmtime;

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

async fn invoke_outgoing_handle<H>(
    handler: H,
    cx: H::Context,
    request: http::Request<HyperOutgoingBody>,
    config: OutgoingRequestConfig,
) -> anyhow::Result<Result<IncomingResponse, types::ErrorCode>>
where
    H: Handler,
{
    use wrpc_interface_http::InvokeOutgoingHandler as _;

    let between_bytes_timeout = config.between_bytes_timeout;
    match handler.invoke_handle_wasmtime(cx, request, config).await? {
        (Ok(resp), errs, io) => {
            let worker = wasmtime_wasi::runtime::spawn(async move {
                // TODO: Do more than just log errors
                join!(
                    errs.for_each(|err| async move {
                        warn!(?err, "body processing error encountered");
                    }),
                    async move {
                        if let Some(io) = io {
                            if let Err(err) = io.await {
                                warn!(?err, "failed to perform async I/O");
                            }
                        }
                    }
                );
            });
            Ok(Ok(IncomingResponse {
                resp,
                worker: Some(worker),
                between_bytes_timeout,
            }))
        }
        (Err(err), _, _) => Ok(Err(err)),
    }
}

impl<H> WasiHttpView for Ctx<H>
where
    H: Handler,
    H::Context: Clone,
{
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn send_request(
        &mut self,
        request: http::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse>
    where
        Self: Sized,
    {
        Ok(HostFutureIncomingResponse::pending(
            wasmtime_wasi::runtime::spawn(invoke_outgoing_handle(
                self.handler.clone(),
                self.cx.clone(),
                request,
                config,
            )),
        ))
    }
}

impl<H, C> ServeIncomingHandlerWasmtime<C> for Instance<H, C>
where
    H: Handler,
    H::Context: Clone,
{
    #[instrument(level = "debug", skip_all)]
    async fn handle(
        &self,
        cx: C,
        request: ::http::Request<wasmtime_wasi_http::body::HyperIncomingBody>,
    ) -> anyhow::Result<
        Result<
            http::Response<wasmtime_wasi_http::body::HyperOutgoingBody>,
            wasmtime_wasi_http::bindings::http::types::ErrorCode,
        >,
    > {
        let (tx, rx) = oneshot::channel();
        let mut store = new_store(
            &self.engine,
            self.handler.clone(),
            self.cx.clone(),
            self.max_execution_time,
        );
        let (bindings, _) =
            incoming_http_bindings::IncomingHttp::instantiate_pre(&mut store, &self.pre).await?;
        let data = store.data_mut();
        let request = data
            .new_incoming_request(request)
            .context("failed to create incoming request")?;
        let response = data
            .new_response_outparam(tx)
            .context("failed to create response")?;
        let handle = spawn(async move {
            bindings
                .wasi_http_incoming_handler()
                .call_handle(&mut store, request, response)
                .await
                .context("failed to call `wasi:http/incoming-handler.handle`")
        });
        let res = async {
            match rx.await {
                Ok(Ok(res)) => Ok(Ok(res)),
                Ok(Err(err)) => Ok(Err(err)),
                Err(_) => {
                    handle.await.context("failed to join handle task")??;
                    bail!("component did not call `response-outparam::set`")
                }
            }
        }
        .await;
        let success = res.is_ok();
        if let Err(err) = self
            .events
            .try_send(WrpcServeEvent::HttpIncomingHandlerHandleReturned {
                context: cx,
                success,
            })
        {
            warn!(
                ?err,
                success, "failed to send `wasi:http/incoming-handler.handle` event"
            )
        }
        res
    }
}
