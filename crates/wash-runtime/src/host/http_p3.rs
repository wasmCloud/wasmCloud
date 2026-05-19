//! P3 HTTP handler: dispatches `wasi:http/handler@0.3` components via
//! wasmtime-wasi-http's `ServicePre`/`Service`. Also exposes the type
//! aliases used at the P3 outgoing-request boundary.

use std::pin::Pin;
use std::task::{Context, Poll};

use crate::engine::ctx::SharedCtx;
use crate::observability::FuelConsumptionMeter;
use http_body_util::BodyExt;
use hyper::body::{Body, Frame, SizeHint};
use tokio::sync::oneshot;
use wasmtime::Store;
use wasmtime::component::InstancePre;
use wasmtime_wasi_http::p3::bindings::ServicePre;
use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

/// Body type used on the P3 outgoing path (also used as the return-body type
/// for [`handle_component_request_p3`]).
pub type P3Body = http_body_util::combinators::UnsyncBoxBody<bytes::Bytes, ErrorCode>;

/// Future returned to the guest to communicate request-side processing errors.
pub type P3RequestErrorFuture = Box<dyn std::future::Future<Output = Result<(), ErrorCode>> + Send>;

/// Result type returned by the inner future of a P3 outgoing send: a response
/// paired with an I/O future for response-body errors.
pub type P3SendResult = Result<
    (hyper::Response<P3Body>, P3RequestErrorFuture),
    wasmtime_wasi::TrappableError<ErrorCode>,
>;

/// Future returned by [`crate::host::http::OutgoingHandler::send_request_p3`].
pub type P3SendFuture = Box<dyn std::future::Future<Output = P3SendResult> + Send>;

/// Streaming body wrapper that signals when hyper is done reading
/// (drained or dropped) so the owning `Store::run_concurrent` can exit.
struct StreamingBody {
    inner: P3Body,
    done: Option<oneshot::Sender<()>>,
}

impl StreamingBody {
    fn new(inner: P3Body) -> (Self, oneshot::Receiver<()>) {
        let (tx, rx) = oneshot::channel();
        (
            Self {
                inner,
                done: Some(tx),
            },
            rx,
        )
    }

    fn signal_done(&mut self) {
        if let Some(tx) = self.done.take() {
            let _ = tx.send(());
        }
    }
}

impl Body for StreamingBody {
    type Data = bytes::Bytes;
    type Error = ErrorCode;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match Pin::new(&mut self.inner).poll_frame(cx) {
            Poll::Ready(None) => {
                self.signal_done();
                Poll::Ready(None)
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for StreamingBody {
    fn drop(&mut self) {
        self.signal_done();
    }
}

/// Handle an HTTP request via the WASIP3 `wasi:http/handler` interface.
///
/// Spawns the request future so `Store::run_concurrent` stays alive for
/// the body's lifetime and chunks stream straight through to hyper.
pub async fn handle_component_request_p3(
    mut store: Store<SharedCtx>,
    pre: InstancePre<SharedCtx>,
    req: hyper::Request<hyper::body::Incoming>,
    fuel_meter: FuelConsumptionMeter,
) -> anyhow::Result<hyper::Response<P3Body>> {
    let _ = &fuel_meter; // fuel metering: see P2's observe() pattern

    let service_pre = ServicePre::new(pre)
        .map_err(|e| anyhow::anyhow!(e).context("failed to create P3 ServicePre"))?;

    let (parts, body) = req.into_parts();
    let body = body
        .map_err(|e| ErrorCode::InternalError(Some(e.to_string())))
        .boxed_unsync();
    let req = hyper::Request::from_parts(parts, body);
    let (wasi_req, req_io) = wasmtime_wasi_http::p3::Request::from_http(req);

    let service = service_pre
        .instantiate_async(&mut store)
        .await
        .map_err(|e| anyhow::anyhow!(e).context("failed to instantiate P3 service"))?;

    let (resp_tx, resp_rx) =
        oneshot::channel::<anyhow::Result<hyper::Response<P3Body>>>();

    tokio::spawn(async move {
        let result = store
            .run_concurrent(async move |store| {
                let io_fut = async {
                    if let Err(e) = req_io.await {
                        tracing::error!(err = ?e, "P3 request I/O error");
                    }
                };

                let handler_fut = async {
                    match service.handle(store, wasi_req).await {
                        Ok(Ok(response)) => {
                            let http_response =
                                store.with(|s| response.into_http(s, async { Ok(()) }))?;
                            let (parts, body) = http_response.into_parts();
                            let (wrapped, done_rx) = StreamingBody::new(body);
                            let body: P3Body = wrapped.boxed_unsync();
                            let hyper_resp = hyper::Response::from_parts(parts, body);

                            if resp_tx.send(Ok(hyper_resp)).is_err() {
                                return Ok::<_, anyhow::Error>(());
                            }
                            // Keep run_concurrent alive while hyper drains.
                            let _ = done_rx.await;
                            Ok(())
                        }
                        Ok(Err(error_code)) => {
                            tracing::error!(?error_code, "P3 HTTP handler returned error");
                            let body: P3Body = http_body_util::Empty::new()
                                .map_err(|never| match never {})
                                .boxed_unsync();
                            let resp = hyper::Response::builder()
                                .status(500)
                                .body(body)
                                .map_err(anyhow::Error::from)?;
                            let _ = resp_tx.send(Ok(resp));
                            Ok(())
                        }
                        Err(e) => {
                            let _ = resp_tx
                                .send(Err(anyhow::anyhow!(e).context("P3 handler trap")));
                            Ok(())
                        }
                    }
                };

                let (handler_result, _) = tokio::join!(handler_fut, io_fut);
                handler_result
            })
            .await;

        if let Err(e) = result {
            tracing::error!(err = ?e, "P3 run_concurrent failed");
        }
    });

    resp_rx
        .await
        .map_err(|_| anyhow::anyhow!("P3 handler task panicked or store was dropped"))?
}
