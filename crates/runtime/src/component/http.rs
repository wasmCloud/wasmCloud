use core::ops::Deref;

use anyhow::{bail, Context as _};
use futures::stream::StreamExt as _;
use tokio::sync::oneshot;
use tokio::{join, spawn};
use tracing::{debug, debug_span, info_span, instrument, trace, warn, Instrument as _, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use wasmtime_wasi::IoView as _;
use wasmtime_wasi_http::body::{HostIncomingBody, HyperOutgoingBody};
use wasmtime_wasi_http::types::{
    HostFutureIncomingResponse, HostIncomingRequest, IncomingResponse, OutgoingRequestConfig,
};
use wasmtime_wasi_http::{HttpResult, WasiHttpCtx, WasiHttpView};
use wrpc_interface_http::ServeIncomingHandlerWasmtime;

use crate::capability::http::types;

use super::{new_store, Ctx, Handler, Instance, ReplacedInstanceTarget, WrpcServeEvent};

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
    debug!(
        target: "runtime::http::outgoing",
        method = %request.method(),
        uri = %request.uri(),
        headers = ?request.headers(),
        connect_timeout = ?config.connect_timeout,
        first_byte_timeout = ?config.first_byte_timeout,
        between_bytes_timeout = ?config.between_bytes_timeout,
        use_tls = ?config.use_tls,
        "Invoking `wrpc:http/outgoing-handler.handle`"
    );

    match handler
        .invoke_handle_wasmtime(
            Some(ReplacedInstanceTarget::HttpOutgoingHandler),
            request,
            config,
        )
        .await?
    {
        (Ok(resp), errs, io) => {
            debug!(
                target: "runtime::http::outgoing",
                status = %resp.status(),
                headers = ?resp.headers(),
                "`wrpc:http/outgoing-handler.handle` succeeded"
            );
            let worker = wasmtime_wasi::runtime::spawn(
                async move {
                    // TODO: Do more than just log errors
                    join!(
                        errs.for_each(|err| async move {
                            warn!(
                                target: "runtime::http::outgoing",
                                ?err,
                                "Body processing error encountered"
                            );
                        }),
                        async move {
                            if let Some(io) = io {
                                debug!(target: "runtime::http::outgoing", "Performing async I/O");
                                if let Err(err) = io.await {
                                    warn!(
                                        target: "runtime::http::outgoing",
                                        ?err,
                                        "Failed to complete async I/O"
                                    );
                                }
                                debug!(target: "runtime::http::outgoing", "Async I/O completed");
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
            warn!(
                target: "runtime::http::outgoing",
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

    #[instrument(level = "debug", skip_all)]
    fn send_request(
        &mut self,
        request: http::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse>
    where
        Self: Sized,
    {
        debug!(
            target: "runtime::http::send_request",
            method = %request.method(),
            uri = %request.uri(),
            headers = ?request.headers(),
            connect_timeout = ?config.connect_timeout,
            first_byte_timeout = ?config.first_byte_timeout,
            between_bytes_timeout = ?config.between_bytes_timeout,
            use_tls = ?config.use_tls,
            "WasiHttpView::send_request called"
        );

        self.attach_parent_context();
        debug!(
            target: "runtime::http::send_request",
            "Parent context attached, spawning request handler"
        );

        // Get the current tracing span
        let current_span = Span::current();

        // Spawn the request handler in the current tracing span
        let future = wasmtime_wasi::runtime::spawn(
            invoke_outgoing_handle(self.handler.clone(), request, config).in_current_span(),
        );

        debug!(
            target: "runtime::http::send_request",
            span_id = ?current_span.id(),
            "Request handler spawned"
        );
        Ok(HostFutureIncomingResponse::pending(future))
    }
}

impl<H, C> ServeIncomingHandlerWasmtime<C> for Instance<H, C>
where
    H: Handler,
    C: Send + Deref<Target = tracing::Span>,
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
        // Set the parent of the current context to the span passed in
        Span::current().set_parent(cx.deref().context());
        let scheme = request.uri().scheme().context("scheme missing")?;
        let scheme = wrpc_interface_http::bindings::wrpc::http::types::Scheme::from(scheme).into();

        let (tx, rx) = oneshot::channel();
        let mut store = new_store(&self.engine, self.handler.clone(), self.max_execution_time);
        let pre = incoming_http_bindings::IncomingHttpPre::new(self.pre.clone())
            .context("failed to pre-instantiate `wasi:http/incoming-handler`")?;
        trace!("instantiating `wasi:http/incoming-handler`");
        let bindings = pre
            .instantiate_async(&mut store)
            .instrument(debug_span!("instantiate_async"))
            .await
            .context("failed to instantiate `wasi:http/incoming-handler`")?;
        let data = store.data_mut();

        // The below is adapted from `WasiHttpView::new_incoming_request`, which is unusable for
        // us, since it requires a `hyper::Error`

        let (parts, body) = request.into_parts();
        let body = HostIncomingBody::new(
            body,
            // TODO: this needs to be plumbed through
            std::time::Duration::from_millis(600 * 1000),
        );
        let incoming_req = HostIncomingRequest::new(data, parts, scheme, Some(body))?;
        let request = data.table().push(incoming_req)?;

        let response = data
            .new_response_outparam(tx)
            .context("failed to create response")?;

        // TODO: Replicate this for custom interface
        // Set the current invocation parent context for injection on outgoing wRPC requests
        let call_incoming_handle = info_span!("call_http_incoming_handle");
        store.data_mut().parent_context = Some(call_incoming_handle.context());
        let handle = spawn(
            async move {
                debug!("invoking `wasi:http/incoming-handler.handle`");
                if let Err(err) = bindings
                    .wasi_http_incoming_handler()
                    .call_handle(&mut store, request, response)
                    .instrument(call_incoming_handle)
                    .await
                {
                    warn!(?err, "failed to call `wasi:http/incoming-handler.handle`");
                    bail!(err.context("failed to call `wasi:http/incoming-handler.handle`"));
                }
                Ok(())
            }
            .in_current_span(),
        );
        let res = async {
            debug!("awaiting `wasi:http/incoming-handler.handle` response");
            match rx.await {
                Ok(Ok(res)) => {
                    debug!("successful `wasi:http/incoming-handler.handle` response received");
                    Ok(Ok(res))
                }
                Ok(Err(err)) => {
                    debug!(
                        ?err,
                        "unsuccessful `wasi:http/incoming-handler.handle` response received"
                    );
                    Ok(Err(err))
                }
                Err(_) => {
                    debug!("`wasi:http/incoming-handler.handle` response sender dropped");
                    handle
                        .instrument(debug_span!("await_response"))
                        .await
                        .context("failed to join handle task")??;
                    bail!("component did not call `response-outparam::set`")
                }
            }
        }
        .in_current_span()
        .await;
        let success = res.as_ref().is_ok_and(Result::is_ok);
        if let Err(err) = self
            .events
            .try_send(WrpcServeEvent::HttpIncomingHandlerHandleReturned {
                context: cx,
                success,
            })
        {
            warn!(
                ?err,
                success, "failed to send `wasi:http/incoming-handler.handle` return event"
            );
        }
        res
    }
}
