//! The [`Ingress::Http`] path: `wasi:http/handler@0.3` requests served on the
//! shared service instance, streaming responses back to the HTTP server.
//!
//! [`Ingress::Http`]: super::Ingress::Http

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use http_body_util::BodyExt;
use wasmtime::component::{Accessor, AccessorTask};
use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode as P2ErrorCode;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
use wasmtime_wasi_http::p3::bindings::Service;
use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

use crate::engine::ctx::SharedCtx;

/// Response body that yields frames forwarded from the [`HttpTask`] over a
/// bounded channel, so a service response streams to the client incrementally
/// instead of being buffered. End-of-stream is signalled when the task drops the
/// sender (body complete, or the task aborted because the client disconnected).
struct ChannelBody {
    rx: tokio::sync::mpsc::Receiver<Result<hyper::body::Frame<bytes::Bytes>, P2ErrorCode>>,
}

impl hyper::body::Body for ChannelBody {
    type Data = bytes::Bytes;
    type Error = P2ErrorCode;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        self.rx.poll_recv(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.rx.is_closed() && self.rx.is_empty()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        if self.is_end_stream() {
            hyper::body::SizeHint::with_exact(0)
        } else {
            hyper::body::SizeHint::default()
        }
    }
}

/// Handles one inbound HTTP request on a shared instance: the workload's
/// service, or one warm instance of a pooled component (see
/// [`crate::engine::instance_driver`]).
///
/// A handler `Err(error-code)` is an ordinary application outcome: this request
/// gets a 500 and the instance keeps serving. A guest *trap* is answered the
/// same way here, but it also faults the store, so the driver's
/// `run_concurrent` returns an error and the service supervisor restarts (and
/// re-registers) a fresh instance. See `test_trigger_service_http_restarts_on_fault`.
pub(crate) struct HttpTask {
    pub(crate) service: Arc<Service>,
    pub(crate) req: hyper::Request<hyper::body::Incoming>,
    pub(crate) resp_tx:
        tokio::sync::oneshot::Sender<anyhow::Result<hyper::Response<HyperOutgoingBody>>>,
    /// Armed by the dispatcher once it has stopped waiting for this response.
    /// It is registered on the store for the life of the call so the epoch
    /// callback can see it — the only way to end a guest that never yields.
    pub(crate) abandoned: std::sync::Arc<crate::engine::abandon::AbandonFlag>,
    /// This call's tether to a pooled instance: holds its in-flight slot and
    /// can retire the instance. `None` for a service, whose singleton instance
    /// is not the pool's to retire.
    pub(crate) pool_slot: Option<crate::engine::instance_driver::PoolSlot>,
}

impl AccessorTask<SharedCtx> for HttpTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        let HttpTask {
            service,
            req,
            resp_tx,
            abandoned,
            pool_slot,
        } = self;

        // The epoch deadline measures this call's own execution, so re-arm it
        // here. `watch_until_abandoned` below owns the registration.
        let calls = accessor.with(|mut access| {
            crate::engine::abandon::rearm_for_call(&mut access);
            std::sync::Arc::clone(&access.get().abandoned)
        });
        // Named from the store this runs on, which was stamped with whose
        // execution it is when it was built — a service's ingress has no
        // workload handle to look one up from. Whether the sample is recorded
        // is `InvocationSample`'s call, not this one's: a service shares its
        // store, so its samples are dropped in favour of the store's own
        // counter, and a pooled instance's are kept when it ran alone.
        let attributes = accessor.with(|mut access| {
            crate::host::http::stored_http_attributes(&access.get().executed, req.method())
        });
        let _sample = crate::engine::instance_driver::InvocationSample::start(attributes);

        let (parts, body) = req.into_parts();
        let body = body
            .map_err(|e| ErrorCode::InternalError(Some(e.to_string())))
            .boxed_unsync();
        let req = hyper::Request::from_parts(parts, body);
        let (wasi_req, req_io) = wasmtime_wasi_http::p3::Request::from_http(req);

        // Bounded channel for response-body frames: the head is delivered to the
        // HTTP server as soon as the handler returns it, while the body keeps
        // streaming. The small capacity back-pressures the guest so frames don't
        // accumulate without bound.
        let (frame_tx, frame_rx) =
            tokio::sync::mpsc::channel::<Result<hyper::body::Frame<bytes::Bytes>, P2ErrorCode>>(4);
        let mut resp_tx = Some(resp_tx);

        let handler_fut = async move {
            let response = match service.handle(accessor, wasi_req).await {
                Ok(Ok(response)) => response,
                Ok(Err(error_code)) => {
                    tracing::error!(?error_code, "service HTTP handler returned error");
                    if let Some(tx) = resp_tx.take() {
                        let resp = hyper::Response::builder()
                            .status(500)
                            .body(HyperOutgoingBody::default())
                            .map_err(anyhow::Error::from);
                        let _ = tx.send(resp);
                    }
                    return Ok(());
                }
                Err(e) => {
                    if let Some(tx) = resp_tx.take() {
                        let _ =
                            tx.send(Err(anyhow::anyhow!(e).context("service HTTP handler trap")));
                    }
                    return Ok(());
                }
            };

            // `into_http`'s future reports the body-delivery outcome back to the
            // guest; resolve it once the body has been fully forwarded.
            let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<Result<(), ErrorCode>>();
            let http_response = match accessor
                .with(|s| response.into_http(s, async move { finish_rx.await.unwrap_or(Ok(())) }))
            {
                Ok(http_response) => http_response,
                Err(e) => {
                    if let Some(tx) = resp_tx.take() {
                        let _ = tx.send(Err(anyhow::anyhow!(e)
                            .context("failed to convert service response to http")));
                    }
                    return Ok(());
                }
            };
            let (head, mut body) = http_response.into_parts();

            // Deliver the head + streaming body to the HTTP server now.
            if let Some(tx) = resp_tx.take() {
                let stream_body =
                    HyperOutgoingBody::new(ChannelBody { rx: frame_rx }.boxed_unsync());
                if tx
                    .send(Ok(hyper::Response::from_parts(head, stream_body)))
                    .is_err()
                {
                    // Caller dropped the receiver; report the failed delivery.
                    let _ = finish_tx.send(Err(ErrorCode::ConnectionTerminated));
                    return Ok(());
                }
            }

            // Forward body frames incrementally; stop if the client disconnects.
            let mut delivery = Ok(());
            while let Some(frame) = body.frame().await {
                // Frames carry the p3 `ErrorCode`; the server body wants the p2 one.
                let frame = frame.map_err(|e| P2ErrorCode::InternalError(Some(format!("{e:?}"))));
                if frame_tx.send(frame).await.is_err() {
                    delivery = Err(ErrorCode::ConnectionTerminated);
                    break;
                }
            }
            let _ = finish_tx.send(delivery);
            Ok::<(), anyhow::Error>(())
        };
        let io_fut = async move {
            let _ = req_io.await;
        };

        // Bound the whole exchange so a stalled client (connected but not reading)
        // can't park this task on `frame_tx.send` for the life of the connection.
        // A response still streaming past this bound is truncated.
        //
        // That bound is far longer than the abandonment grace, so keeping a
        // slow guest away from the epoch callback is `watch_until_abandoned`'s
        // job, not this one's.
        match tokio::time::timeout(
            crate::timeouts::http_response(),
            crate::engine::abandon::watch_until_abandoned(
                &calls,
                abandoned,
                futures::future::join(handler_fut, io_fut),
            ),
        )
        .await
        {
            Ok((handler_result, ())) => {
                if let Err(e) = handler_result {
                    tracing::error!(err = ?e, "service HTTP response streaming failed");
                }
            }
            // The guest work behind the timed-out exchange cannot be cancelled
            // from the host. On a pooled instance the remedy is retirement:
            // stop admitting, drain, and let the store's teardown end the
            // stalled work. A service has no such remedy — its singleton
            // instance keeps serving, with the stalled task still on it — so
            // the timeout only bounds how long the client waits.
            //
            // TODO: both arms want per-task cancellation
            // (bytecodealliance/wasmtime#11833). With it, a pooled instance
            // would cancel the one bad call instead of being condemned, and a
            // service would shed its stalled task instead of carrying it for
            // the rest of its life.
            Err(_) => match &pool_slot {
                Some(slot) => {
                    slot.retire_instance();
                    tracing::error!(
                        "HTTP call timed out; retiring its pooled instance to end the stalled work"
                    );
                }
                None => tracing::error!("service HTTP response timed out"),
            },
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use hyper::body::Frame;

    /// [`ChannelBody`] must report end-of-stream **at rest** — closed sender,
    /// drained buffer — not only through a terminal poll: hyper stops polling
    /// a fixed-length body early, and `WatchedBody` reads this to tell a
    /// delivered response from an abandoned one.
    #[tokio::test]
    async fn channel_body_reports_end_of_stream_at_rest() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, P2ErrorCode>>(4);
        let mut body = ChannelBody { rx };
        assert!(!hyper::body::Body::is_end_stream(&body));

        tx.send(Ok(Frame::data(Bytes::from_static(b"data"))))
            .await
            .expect("send");
        drop(tx);
        // Closed but not yet drained: the last frame is still deliverable.
        assert!(!hyper::body::Body::is_end_stream(&body));

        let frame = std::future::poll_fn(|cx| {
            hyper::body::Body::poll_frame(std::pin::Pin::new(&mut body), cx)
        })
        .await
        .expect("a frame")
        .expect("not an error");
        assert!(frame.is_data());
        // Closed and drained: ended, with an exact size of zero.
        assert!(hyper::body::Body::is_end_stream(&body));
        assert_eq!(hyper::body::Body::size_hint(&body).exact(), Some(0));
    }

    /// [`ChannelBody`] must forward frames **incrementally** — a consumer
    /// receives a frame while the producer is still parked — so a service
    /// response streams to the client rather than being buffered whole. Also
    /// checks that end-of-stream is signalled when the producer drops its sender.
    #[tokio::test]
    async fn channel_body_streams_frames_incrementally() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Frame<Bytes>, P2ErrorCode>>(4);
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

        let mut body = ChannelBody { rx };

        let first = body
            .frame()
            .await
            .expect("a frame")
            .expect("ok frame")
            .into_data()
            .expect("data frame");
        assert_eq!(first.as_ref(), b"first");

        // Receiving `first` while the producer is still parked on `ack_rx` proves
        // incremental (non-buffered) delivery. Release the producer for the rest.
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
        producer.await.expect("producer task");
    }
}
