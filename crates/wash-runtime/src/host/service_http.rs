//! Co-driver for a **service that also exports `wasi:http/handler`**.
//!
//! A wasmCloud service is a long-lived `wasi:cli/run` component. When that same
//! component also exports `wasi:http/handler@0.3`, this driver runs BOTH on a
//! single instance, under one [`Store::run_concurrent`]:
//!
//! - the `cli/run` export drives the service's own long-running work (e.g. a
//!   connection pooler listening on a loopback socket), and
//! - inbound HTTP is delivered over a channel and each request is handled by
//!   the `wasi:http/handler` export via [`Accessor::spawn`].
//!
//! This is what lets one Service be both the stateful egress (the pool) and the
//! HTTP ingress/router for the rest of the workload, on the same instance.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use http_body_util::BodyExt;
use wasmtime::Store;
use wasmtime::component::{Accessor, AccessorTask, InstancePre};
use wasmtime_wasi::p3::bindings::Command;
use wasmtime_wasi_http::p2::body::HyperOutgoingBody;
// The p2 and p3 `ErrorCode`s are distinct types: `HyperOutgoingBody` (the body
// type the HTTP server expects) carries the p2 one, while the p3 handler and
// `into_http` use the p3 one.
use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode as P2ErrorCode;
use wasmtime_wasi_http::p3::bindings::Service;
use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;

use crate::engine::ctx::SharedCtx;
use crate::host::http::ServiceHttpJob;

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
}

/// A running service instance that serves HTTP ingress from its
/// `wasi:http/handler` export while its `cli/run` export keeps running.
pub struct ServiceHttpService {
    /// Send inbound HTTP here; the paired oneshot receives the response.
    pub sender: tokio::sync::mpsc::Sender<ServiceHttpJob>,
    /// The driver task: instantiates once and runs cli/run + HTTP concurrently.
    pub driver: tokio::task::JoinHandle<()>,
}

impl ServiceHttpService {
    /// Instantiate the service once and start driving both its `cli/run` and
    /// `wasi:http/handler` exports on the same instance.
    pub fn spawn(
        mut store: Store<SharedCtx>,
        pre: InstancePre<SharedCtx>,
    ) -> anyhow::Result<Self> {
        let (sender, mut rx) = tokio::sync::mpsc::channel::<ServiceHttpJob>(256);

        let driver = tokio::spawn(async move {
            // One instance, two views: the cli `Command` and the http `Service`.
            let instance = match pre.instantiate_async(&mut store).await {
                Ok(i) => i,
                Err(e) => {
                    tracing::error!(err = %e, "failed to instantiate HTTP service");
                    return;
                }
            };
            let command = match Command::new(&mut store, &instance) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(err = %e, "service is missing wasi:cli/run export");
                    return;
                }
            };
            let service = match Service::new(&mut store, &instance) {
                Ok(s) => std::sync::Arc::new(s),
                Err(e) => {
                    tracing::error!(err = %e, "service is missing wasi:http/handler export");
                    return;
                }
            };

            let result = store
                .run_concurrent(async move |accessor| {
                    // Drive the service's own long-running work (e.g. the pooler).
                    accessor.spawn(RunTask { command });
                    // Serve inbound HTTP: one concurrent task per request, all on
                    // this same instance.
                    while let Some((req, resp_tx)) = rx.recv().await {
                        accessor.spawn(HttpTask {
                            service: std::sync::Arc::clone(&service),
                            req,
                            resp_tx,
                        });
                    }
                    Ok::<(), anyhow::Error>(())
                })
                .await;
            if let Err(e) = result {
                tracing::error!(err = %e, "HTTP service driver exited");
            }
        });

        Ok(ServiceHttpService { sender, driver })
    }
}

/// Drives the service's `wasi:cli/run` export (its long-running work).
struct RunTask {
    command: Command,
}

impl AccessorTask<SharedCtx> for RunTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        match self.command.wasi_cli_run().call_run(accessor).await {
            Ok(Ok(())) => tracing::info!("service cli/run exited successfully"),
            Ok(Err(())) => tracing::error!("service cli/run exited with error"),
            Err(e) => tracing::error!(err = %e, "service cli/run trapped"),
        }
        Ok(())
    }
}

/// Handles one inbound HTTP request on the shared service instance.
struct HttpTask {
    service: std::sync::Arc<Service>,
    req: hyper::Request<hyper::body::Incoming>,
    resp_tx: tokio::sync::oneshot::Sender<anyhow::Result<hyper::Response<HyperOutgoingBody>>>,
}

impl AccessorTask<SharedCtx> for HttpTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        let HttpTask {
            service,
            req,
            resp_tx,
        } = self;

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
                        let _ = tx.send(Err(anyhow::anyhow!(e).context("service HTTP handler trap")));
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
                        let _ = tx.send(Err(
                            anyhow::anyhow!(e).context("failed to convert service response to http")
                        ));
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
        const RESPONSE_TIMEOUT: Duration = Duration::from_secs(600);
        match tokio::time::timeout(
            RESPONSE_TIMEOUT,
            futures::future::join(handler_fut, io_fut),
        )
        .await
        {
            Ok((handler_result, ())) => {
                if let Err(e) = handler_result {
                    tracing::error!(err = ?e, "service HTTP response streaming failed");
                }
            }
            Err(_) => tracing::error!("service HTTP response timed out"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use hyper::body::Frame;

    /// [`ChannelBody`] must forward frames **incrementally** — a consumer
    /// receives a frame while the producer is still parked — so a service
    /// response streams to the client rather than being buffered whole. Also
    /// checks that end-of-stream is signalled when the producer drops its sender.
    #[tokio::test]
    async fn channel_body_streams_frames_incrementally() {
        let (tx, rx) =
            tokio::sync::mpsc::channel::<Result<Frame<Bytes>, P2ErrorCode>>(4);
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
