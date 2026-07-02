//! Reactor: a long-lived service instance that co-drives `wasi:cli/run`
//! alongside one or more host-invoked handler exports on a single instance.
//!
//! A wasmCloud service is a long-lived `wasi:cli/run` component. When that same
//! component also exports a host-invoked handler (today `wasi:http/handler@0.3`),
//! the Reactor runs BOTH on one instance under a single [`Store::run_concurrent`]:
//!
//! - the `cli/run` export drives the service's own long-running work (e.g. a
//!   connection pooler listening on a loopback socket), and
//! - each host-invoked export is served as concurrent per-invocation tasks on
//!   the same instance, so the long-running work and the handlers share the
//!   instance's in-memory state.
//!
//! Each host-invoked export is an [`Ingress`]: the host-side plugin (the HTTP
//! server, ...) pushes invocations into the ingress's channel and the Reactor
//! serves them via [`Accessor::spawn`]. Adding another host-invoked interface
//! (e.g. a messaging handler) is a new [`Ingress`] variant plus a serve arm —
//! the `cli/run` driving and the single-instance `run_concurrent` are reused.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use http_body_util::BodyExt;
use wasmtime::Store;
use wasmtime::component::{
    Accessor, AccessorTask, ComponentExportIndex, Instance, InstancePre, Val,
};
use wasmtime::error::Context as _;
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

/// An inbound message delivered to the service's `wasmcloud:messaging/handler`
/// export. Mirrors the `broker-message` record.
pub struct BrokerMessage {
    pub subject: String,
    pub body: Vec<u8>,
    pub reply_to: Option<String>,
}

/// A messaging invocation: the message plus a oneshot carrying the handler's
/// `result<_, string>` outcome back to the host-side ingress (to ack/log).
pub type MessagingJob = (BrokerMessage, tokio::sync::oneshot::Sender<Result<(), String>>);

/// A host-invoked handler export the Reactor serves, carrying the receiver end
/// of its delivery channel. The paired sender is handed to the host-side ingress
/// (the HTTP server, the messaging subscriber, ...) so it can deliver
/// invocations to this live instance.
pub enum Ingress {
    /// `wasi:http/handler@0.3` — the HTTP server delivers requests here.
    Http(tokio::sync::mpsc::Receiver<ServiceHttpJob>),
    /// `wasmcloud:messaging/handler@0.2.0` — the messaging subscriber delivers
    /// received messages here.
    Messaging(tokio::sync::mpsc::Receiver<MessagingJob>),
}

/// Interface + function names for the messaging handler export.
const MESSAGING_HANDLER: &str = "wasmcloud:messaging/handler@0.2.0";
const HANDLE_MESSAGE: &str = "handle-message";

impl Ingress {
    /// Build this ingress's binding view over the shared instance. Done before
    /// `run_concurrent` (which needs `&mut store`), mirroring the `cli` view.
    fn prepare(
        self,
        store: &mut Store<SharedCtx>,
        instance: &Instance,
    ) -> anyhow::Result<PreparedIngress> {
        match self {
            Ingress::Http(rx) => {
                let service = Service::new(store, instance)
                    .map_err(|e| e.context("service is missing wasi:http/handler export"))?;
                Ok(PreparedIngress::Http {
                    service: Arc::new(service),
                    rx,
                })
            }
            Ingress::Messaging(rx) => {
                // Look up the p2 `handle-message` export up front; it's invoked
                // dynamically (there is no accessor-driven p3 messaging binding).
                let iface = instance
                    .get_export(&mut *store, None, MESSAGING_HANDLER)
                    .with_context(|| format!("service is missing {MESSAGING_HANDLER} export"))?
                    .1;
                let func_idx = instance
                    .get_export(&mut *store, Some(&iface), HANDLE_MESSAGE)
                    .with_context(|| format!("{MESSAGING_HANDLER} is missing {HANDLE_MESSAGE}"))?
                    .1;
                Ok(PreparedIngress::Messaging {
                    instance: *instance,
                    func_idx,
                    rx,
                })
            }
        }
    }
}

/// An [`Ingress`] with its binding view built, ready to serve invocations under
/// `run_concurrent`.
enum PreparedIngress {
    Http {
        service: Arc<Service>,
        rx: tokio::sync::mpsc::Receiver<ServiceHttpJob>,
    },
    Messaging {
        instance: Instance,
        func_idx: ComponentExportIndex,
        rx: tokio::sync::mpsc::Receiver<MessagingJob>,
    },
}

impl PreparedIngress {
    /// Serve inbound invocations until the delivery channel closes, spawning one
    /// concurrent task per invocation on the shared instance.
    async fn serve(self, accessor: &Accessor<SharedCtx>) {
        match self {
            PreparedIngress::Http { service, mut rx } => {
                while let Some((req, resp_tx)) = rx.recv().await {
                    accessor.spawn(HttpTask {
                        service: Arc::clone(&service),
                        req,
                        resp_tx,
                    });
                }
            }
            PreparedIngress::Messaging {
                instance,
                func_idx,
                mut rx,
            } => {
                while let Some((msg, result_tx)) = rx.recv().await {
                    accessor.spawn(MessagingTask {
                        instance,
                        func_idx,
                        msg,
                        result_tx,
                    });
                }
            }
        }
    }
}

/// A running service instance co-driving `cli/run` and its host-invoked handler
/// exports on one instance.
pub struct Reactor {
    /// The driver task: instantiates once and runs cli/run + every ingress
    /// concurrently.
    pub driver: tokio::task::JoinHandle<()>,
}

impl Reactor {
    /// Instantiate the service once and start driving its `cli/run` export plus
    /// every `ingress` on the same instance under one `run_concurrent`.
    pub fn spawn(
        mut store: Store<SharedCtx>,
        pre: InstancePre<SharedCtx>,
        ingresses: Vec<Ingress>,
    ) -> anyhow::Result<Self> {
        let driver = tokio::spawn(async move {
            let instance = match pre.instantiate_async(&mut store).await {
                Ok(i) => i,
                Err(e) => {
                    tracing::error!(err = %e, "failed to instantiate reactor service");
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
            // Build each ingress's binding view before entering run_concurrent.
            let mut prepared = Vec::with_capacity(ingresses.len());
            for ingress in ingresses {
                match ingress.prepare(&mut store, &instance) {
                    Ok(p) => prepared.push(p),
                    Err(e) => {
                        tracing::error!(err = %e, "failed to prepare reactor ingress");
                        return;
                    }
                }
            }

            let result = store
                .run_concurrent(async move |accessor| {
                    // Drive the service's own long-running work (e.g. the pooler).
                    accessor.spawn(RunTask { command });
                    // Serve every ingress concurrently on this same instance; the
                    // driver runs until all ingress channels close (workload stop).
                    futures::future::join_all(prepared.into_iter().map(|p| p.serve(accessor)))
                        .await;
                    Ok::<(), anyhow::Error>(())
                })
                .await;
            if let Err(e) = result {
                tracing::error!(err = %e, "reactor driver exited");
            }
        });

        Ok(Reactor { driver })
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

/// Handles one inbound message on the shared service instance by invoking the
/// p2 `handle-message` export via the dynamic concurrent path (there is no
/// accessor-driven p3 messaging binding), and reports its `result<_, string>`.
struct MessagingTask {
    instance: Instance,
    func_idx: ComponentExportIndex,
    msg: BrokerMessage,
    result_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
}

impl AccessorTask<SharedCtx> for MessagingTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        let MessagingTask {
            instance,
            func_idx,
            msg,
            result_tx,
        } = self;

        let func = match accessor.with(|mut store| instance.get_func(&mut store, func_idx)) {
            Some(func) => func,
            None => {
                let _ = result_tx.send(Err("handle-message export not found".to_string()));
                return Ok(());
            }
        };

        // Lower the `broker-message` record to a `Val`.
        let message = Val::Record(vec![
            ("subject".to_string(), Val::String(msg.subject)),
            (
                "body".to_string(),
                Val::List(msg.body.into_iter().map(Val::U8).collect()),
            ),
            (
                "reply-to".to_string(),
                Val::Option(msg.reply_to.map(|s| Box::new(Val::String(s)))),
            ),
        ]);

        let mut results = vec![Val::Bool(false)];
        let outcome = match func.call_concurrent(accessor, &[message], &mut results).await {
            Ok(()) => lift_handle_result(results.first()),
            Err(e) => Err(format!("handle-message trapped: {e:#}")),
        };
        let _ = result_tx.send(outcome);
        Ok(())
    }
}

/// Lift the `result<_, string>` returned by `handle-message`.
fn lift_handle_result(v: Option<&Val>) -> Result<(), String> {
    match v {
        Some(Val::Result(Ok(_))) => Ok(()),
        Some(Val::Result(Err(Some(boxed)))) => match &**boxed {
            Val::String(s) => Err(s.clone()),
            other => Err(format!("{other:?}")),
        },
        Some(Val::Result(Err(None))) => Err(String::new()),
        other => Err(format!("unexpected handle-message result: {other:?}")),
    }
}

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

/// Handles one inbound HTTP request on the shared service instance.
struct HttpTask {
    service: Arc<Service>,
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
