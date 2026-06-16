//! P3 HTTP handler for WASIP3 components.
//!
//! This module provides the HTTP request handling path for components that
//! target WASIP3's `wasi:http/handler` interface. It uses wasmtime-wasi-http's
//! P3 `ServicePre`/`Service` to invoke the component.
//!
//! It also exposes the type aliases used at the P3 outgoing-request boundary
//! ([`P3Body`], [`P3RequestErrorFuture`], [`P3SendFuture`]). The
//! outgoing-request egress policy itself lives on the unified
//! [`crate::host::http::OutgoingHandler`] trait via its `send_request_p3` method.

use crate::engine::ctx::SharedCtx;
use crate::observability::FuelConsumptionMeter;
use http_body_util::BodyExt;
use tracing::Instrument;
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

/// Called once the P3 component task exits — whether it ran to completion,
/// returned early, or was aborted because the response body was dropped (client
/// disconnect). Used to reclaim per-request store state, e.g. the cached
/// linked-component instances keyed by this request's store id.
pub type P3CompletionHook = Box<dyn FnOnce() + Send + 'static>;

/// Runs the [`P3CompletionHook`] from `Drop` so it fires on **every** task exit
/// path. The component runs in a task wrapped in an `AbortOnDropHandle`, so when
/// the response body is dropped mid-stream the task is cancelled and any
/// end-of-task cleanup code would never run — a `Drop` guard does.
struct OnCompleteGuard(Option<P3CompletionHook>);

impl Drop for OnCompleteGuard {
    fn drop(&mut self) {
        if let Some(hook) = self.0.take() {
            hook();
        }
    }
}

/// Response body that yields frames forwarded from the component task over a
/// bounded channel. End-of-stream is signalled when the sender (held by the
/// component task) is dropped.
struct ChannelBody {
    rx: tokio::sync::mpsc::Receiver<Result<hyper::body::Frame<bytes::Bytes>, ErrorCode>>,
    /// Aborts the component task when this body is dropped before the stream
    /// completes (e.g. the client disconnects). Without this, a guest that is
    /// busy computing — rather than parked on a frame send — would keep running
    /// until its next send; there is no epoch/wall-clock backstop. Held only
    /// for its `Drop`.
    _task: tokio_util::task::AbortOnDropHandle<()>,
}

impl hyper::body::Body for ChannelBody {
    type Data = bytes::Bytes;
    type Error = ErrorCode;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        self.rx.poll_recv(cx)
    }
}

/// Handle an HTTP request using the WASIP3 `wasi:http/handler` interface.
///
/// P3 uses `ServicePre`/`Service` with `Store::run_concurrent` to get an
/// `Accessor` for concurrent component-model async operations.
///
/// The response body is **streamed**: the component runs in a background task
/// that owns the store and drives `run_concurrent`, forwarding response frames
/// over a bounded channel as they are produced. The store and therefore the
/// guest stays alive until the body has been fully drained, and a slow client
/// applies backpressure to the guest rather than buffering the whole body in
/// memory.
///
/// `on_complete` runs once the component task exits (completion, early error, or
/// abort-on-disconnect) to reclaim per-request store state.
pub async fn handle_component_request_p3(
    mut store: Store<SharedCtx>,
    pre: InstancePre<SharedCtx>,
    req: hyper::Request<hyper::body::Incoming>,
    fuel_meter: FuelConsumptionMeter,
    on_complete: P3CompletionHook,
) -> anyhow::Result<hyper::Response<P3Body>> {
    let _ = &fuel_meter; // fuel metering integration deferred to match P2's observe() pattern

    let service_pre = match ServicePre::new(pre)
        .map_err(|e| anyhow::anyhow!(e).context("failed to create P3 ServicePre"))
    {
        Ok(service_pre) => service_pre,
        Err(e) => {
            // No task is spawned on this path, so run the hook here.
            on_complete();
            return Err(e);
        }
    };

    // Convert the hyper request body — map error type since hyper::Error doesn't impl Into<ErrorCode>
    let (parts, body) = req.into_parts();
    let body = body
        .map_err(|e| ErrorCode::InternalError(Some(e.to_string())))
        .boxed_unsync();
    let req = hyper::Request::from_parts(parts, body);
    let (wasi_req, req_io) = wasmtime_wasi_http::p3::Request::from_http(req);

    // Bounded so a slow client applies backpressure to the guest instead of
    // letting response frames accumulate without limit.
    let (frame_tx, frame_rx) =
        tokio::sync::mpsc::channel::<Result<hyper::body::Frame<bytes::Bytes>, ErrorCode>>(4);
    // Delivers the response head (status + headers) as soon as the handler
    // returns it, while the body is still streaming.
    let (parts_tx, parts_rx) =
        tokio::sync::oneshot::channel::<anyhow::Result<hyper::http::response::Parts>>();

    // The store is owned by this task for the entire response lifetime.
    // `run_concurrent` only resolves once every frame has been forwarded, so the
    // guest keeps running until the body is fully drained. Wrapped in an
    // `AbortOnDropHandle` immediately (not just once the body holds it) so that
    // if this request future is cancelled while still awaiting the response head
    // — e.g. the client disconnects before the guest produces a response — the
    // task is aborted rather than detached to run unbounded.
    let task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
        async move {
            // Reclaim per-request store state on every exit path below
            // (completion, early return, or abort), via this guard's `Drop`.
            let _on_complete = OnCompleteGuard(Some(on_complete));

            let service = match service_pre.instantiate_async(&mut store).await {
                Ok(service) => service,
                Err(e) => {
                    let _ = parts_tx.send(Err(
                        anyhow::anyhow!(e).context("failed to instantiate P3 service")
                    ));
                    return;
                }
            };

            let run = store
                .run_concurrent(async move |store| {
                    let mut parts_tx = Some(parts_tx);
                    // `async move` so `handler_fut` owns `frame_tx`: it drops the
                    // instant the response body completes, delivering end-of-stream
                    // to the client without waiting for `io_fut` (request-body
                    // upload) to finish in the `join!` below.
                    let handler_fut = async move {
                        let response = match service.handle(store, wasi_req).await {
                            Ok(Ok(response)) => response,
                            Ok(Err(error_code)) => {
                                tracing::error!(?error_code, "P3 HTTP handler returned error");
                                if let Some(tx) = parts_tx.take() {
                                    let (mut head, ()) = hyper::Response::new(()).into_parts();
                                    head.status = hyper::StatusCode::INTERNAL_SERVER_ERROR;
                                    let _ = tx.send(Ok(head));
                                }
                                return Ok(());
                            }
                            Err(e) => {
                                if let Some(tx) = parts_tx.take() {
                                    let _ =
                                        tx.send(Err(anyhow::anyhow!(e).context("P3 handler trap")));
                                }
                                return Ok(());
                            }
                        };
                        // `into_http`'s `fut` reports the body-delivery outcome
                        // back to the guest (it resolves the future returned by
                        // `wasi:http/types.response#new`). Resolve it once we have
                        // finished forwarding the body to the client, so a guest
                        // that awaits the result learns whether delivery succeeded.
                        let (finish_tx, finish_rx) =
                            tokio::sync::oneshot::channel::<Result<(), ErrorCode>>();
                        let http_response = match store.with(|s| {
                            response.into_http(s, async move {
                                match finish_rx.await {
                                    Ok(result) => {
                                        tracing::trace!(
                                            ?result,
                                            "P3 response body delivery finished"
                                        );
                                        result
                                    }
                                    // Sender dropped (task aborted mid-stream),
                                    // e.g. the client disconnected; nothing to
                                    // report back to the guest.
                                    Err(_) => {
                                        tracing::debug!(
                                            "P3 response delivery future dropped before completion"
                                        );
                                        Ok(())
                                    }
                                }
                            })
                        }) {
                            Ok(http_response) => http_response,
                            Err(e) => {
                                if let Some(tx) = parts_tx.take() {
                                    let _ = tx.send(Err(anyhow::anyhow!(e)
                                        .context("failed to convert P3 response to http")));
                                }
                                return Ok(());
                            }
                        };
                        let (head, mut body) = http_response.into_parts();
                        if let Some(tx) = parts_tx.take()
                            && tx.send(Ok(head)).is_err()
                        {
                            // Caller dropped the receiver; report the failed
                            // delivery to the guest and stop.
                            let _ = finish_tx.send(Err(ErrorCode::ConnectionTerminated));
                            return Ok(());
                        }
                        let mut delivery = Ok(());
                        while let Some(frame) = body.frame().await {
                            if frame_tx.send(frame).await.is_err() {
                                // The hyper response body was dropped (e.g. the
                                // client disconnected); stop pulling from the guest
                                // and report the failed delivery.
                                delivery = Err(ErrorCode::ConnectionTerminated);
                                break;
                            }
                        }
                        let _ = finish_tx.send(delivery);
                        Ok::<(), anyhow::Error>(())
                    };
                    let io_fut = async {
                        if let Err(e) = req_io.await {
                            tracing::error!(err = ?e, "P3 request I/O error");
                        }
                    };
                    let (handler_result, ()) = tokio::join!(handler_fut, io_fut);
                    handler_result
                })
                .await;
            match run {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::error!(err = ?e, "P3 response streaming failed"),
                Err(e) => tracing::error!(err = ?e, "P3 run_concurrent failed"),
            }
        }
        .in_current_span(),
    ));

    let head = parts_rx
        .await
        .map_err(|_| anyhow::anyhow!("P3 component task ended before producing a response"))??;
    let body: P3Body = ChannelBody {
        rx: frame_rx,
        _task: task,
    }
    .boxed_unsync();
    Ok(hyper::Response::from_parts(head, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use hyper::body::Frame;

    /// `ChannelBody` must forward frames **incrementally**, a consumer should
    /// receive a frame while the producer is still parked, not only after the
    /// producer has finished and signal end-of-stream when the producer drops
    /// its sender.
    #[tokio::test]
    async fn channel_body_streams_frames_incrementally() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, ErrorCode>>(4);
        // Gates the producer's second frame on the consumer acknowledging the
        // first, proving the first was delivered before the producer completed.
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<()>();

        let producer = tokio::spawn(async move {
            tx.send(Ok(Frame::data(Bytes::from_static(b"first"))))
                .await
                .expect("send first");
            ack_rx.await.expect("consumer ack");
            tx.send(Ok(Frame::data(Bytes::from_static(b"second"))))
                .await
                .expect("send second");
            // `tx` dropped here -> end-of-stream.
        });

        let mut body = ChannelBody {
            rx,
            _task: tokio_util::task::AbortOnDropHandle::new(producer),
        };

        let first = body
            .frame()
            .await
            .expect("a frame")
            .expect("ok frame")
            .into_data()
            .expect("data frame");
        assert_eq!(first.as_ref(), b"first");

        // The producer is now parked on `ack_rx`; releasing it lets the second
        // frame flow. Receiving `first` before this proves incremental delivery.
        ack_tx.send(()).expect("release producer");

        let second = body
            .frame()
            .await
            .expect("a frame")
            .expect("ok frame")
            .into_data()
            .expect("data frame");
        assert_eq!(second.as_ref(), b"second");

        assert!(
            body.frame().await.is_none(),
            "stream should end when the producer drops its sender"
        );
    }

    /// Dropping a `ChannelBody` must abort the component task (via the held
    /// `AbortOnDropHandle`) so the guest is cancelled promptly on disconnect.
    #[tokio::test]
    async fn channel_body_drop_aborts_task() {
        let (_tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, ErrorCode>>(1);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let (gone_tx, gone_rx) = tokio::sync::oneshot::channel::<()>();

        let task = tokio::spawn(async move {
            let _gone_tx = gone_tx; // dropped (closing the channel) only when the task ends
            let _ = started_tx.send(());
            // Runs "forever" unless aborted.
            std::future::pending::<()>().await;
        });

        let body = ChannelBody {
            rx,
            _task: tokio_util::task::AbortOnDropHandle::new(task),
        };
        started_rx.await.expect("task started");

        drop(body);

        // The task's `gone_tx` is dropped when the task is aborted, so this
        // resolves with a recv error.
        assert!(
            gone_rx.await.is_err(),
            "dropping the body should abort the task"
        );
    }
}
