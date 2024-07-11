use super::{new_store, Ctx, Handler, Instance, ReplacedInstanceTarget, WrpcServeEvent};

use crate::capability::http::types;

use anyhow::{bail, Context as _};
use futures::stream::StreamExt as _;
use tokio::sync::oneshot;
use tokio::{join, spawn};
use tracing::{debug, instrument, warn, Instrument as _};
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

#[instrument(level = "debug", skip_all)]
async fn invoke_outgoing_handle<H>(
    handler: H,
    request: http::Request<HyperOutgoingBody>,
    config: OutgoingRequestConfig,
) -> anyhow::Result<Result<IncomingResponse, types::ErrorCode>>
where
    H: Handler,
{
    use wrpc_interface_http::InvokeOutgoingHandler as _;

    let between_bytes_timeout = config.between_bytes_timeout;
    debug!("invoking `wrpc:http/outgoing-handler.handle`");
    match handler
        .invoke_handle_wasmtime(
            Some(ReplacedInstanceTarget::HttpOutgoingHandler),
            request,
            config,
        )
        .await?
    {
        (Ok(resp), errs, io) => {
            debug!("`wrpc:http/outgoing-handler.handle` succeeded");
            let worker = wasmtime_wasi::runtime::spawn(
                async move {
                    // TODO: Do more than just log errors
                    join!(
                        errs.for_each(|err| async move {
                            warn!(?err, "body processing error encountered");
                        }),
                        async move {
                            if let Some(io) = io {
                                debug!("performing async I/O");
                                if let Err(err) = io.await {
                                    warn!(?err, "failed to complete async I/O");
                                }
                                debug!("async I/O completed");
                            }
                        }
                    );
                }
                .in_current_span(),
            );
            Ok(Ok(IncomingResponse {
                resp,
                worker: Some(worker),
                between_bytes_timeout,
            }))
        }
        (Err(err), _, _) => {
            debug!(
                ?err,
                "`wrpc:http/outgoing-handler.handle` returned an error code"
            );
            Ok(Err(err))
        }
    }
}

impl<H> WasiHttpView for Ctx<H>
where
    H: Handler,
{
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    #[instrument(level = "debug", skip_all)]
    fn send_request(
        &mut self,
        request: http::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse>
    where
        Self: Sized,
    {
        Ok(HostFutureIncomingResponse::pending(
            wasmtime_wasi::runtime::spawn(
                invoke_outgoing_handle(self.handler.clone(), request, config).in_current_span(),
            ),
        ))
    }
}

impl<H, C> ServeIncomingHandlerWasmtime<C> for Instance<H, C>
where
    H: Handler,
    C: Send,
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
        let mut store = new_store(&self.engine, self.handler.clone(), self.max_execution_time);
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
