//! P3 HTTP handler for WASIP3 components.
//!
//! This module provides the HTTP request handling path for components that
//! target WASIP3's `wasi:http/handler` interface. It uses wasmtime-wasi-http's
//! P3 `ServicePre`/`Service` to invoke the component.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::engine::instance_pool::ComponentInstance;
use crate::host::http::ResponseBodyCompletion;
use futures::task::AtomicWaker;
use http_body_util::BodyExt;
use tracing::Instrument;
use wasmtime_wasi_http::p3::bindings::Service;
use wasmtime_wasi_http::{Error, WasiBody};

/// A guest's response body handed to the HTTP server as it is, reporting how
/// its delivery ended to the paired [`Delivery`].
///
/// The body `into_http` returns is already a channel the store feeds, so the
/// server reads it directly. Copying its frames into another channel from a
/// task on the store would cost the store's event loop extra turns per frame.
pub(crate) struct DeliveredBody {
    /// `None` once a cut took effect.
    body: Option<WasiBody>,
    completion: ResponseBodyCompletion,
    cut: Arc<Cut>,
    /// `None` once the outcome was reported.
    done: Option<tokio::sync::oneshot::Sender<Result<(), Error>>>,
    /// Aborts the component task when this body is dropped before the stream
    /// completes (e.g. the client disconnects). The abort lands at the guest's
    /// next await, reclaiming a yielding guest at once; a guest that never
    /// yields is unreachable by it and is ended by its abandoned call instead
    /// (see [`crate::engine::abandon`]). Held only for its `Drop`.
    task: Option<tokio_util::task::AbortOnDropHandle<()>>,
}

/// The serving task's side of a [`DeliveredBody`]: how its delivery ended,
/// and the means to end it early.
///
/// Dropping it before delivery ends cuts the response short: the body ends at
/// its next poll, releasing the guest's stream, and a server parked waiting
/// on the body is woken to take that poll. A server that has stopped polling
/// because its client stopped reading keeps the body until the connection
/// goes.
pub(crate) struct Delivery {
    cut: Arc<Cut>,
    done: tokio::sync::oneshot::Receiver<Result<(), Error>>,
    ended: bool,
}

#[derive(Default)]
struct Cut {
    requested: AtomicBool,
    /// The server's waker, registered only while the body is pending.
    waker: AtomicWaker,
}

impl DeliveredBody {
    /// An empty body no task waits on.
    pub(crate) fn empty() -> Self {
        Self {
            body: Some(WasiBody::default()),
            completion: ResponseBodyCompletion::Stream,
            cut: Arc::default(),
            done: None,
            task: None,
        }
    }

    pub(crate) fn new(body: WasiBody, completion: ResponseBodyCompletion) -> (Self, Delivery) {
        let cut = Arc::new(Cut::default());
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let delivered = Self {
            body: Some(body),
            completion,
            cut: Arc::clone(&cut),
            done: Some(done_tx),
            task: None,
        };
        (
            delivered,
            Delivery {
                cut,
                done: done_rx,
                ended: false,
            },
        )
    }

    fn report(&mut self, delivery: Result<(), Error>) {
        if let Some(done) = self.done.take() {
            let _ = done.send(delivery);
        }
    }

    /// Drop the guest's body if the serving task cut the response short.
    fn take_cut(&mut self) -> bool {
        if self.cut.requested.load(Ordering::Acquire) {
            self.body = None;
        }
        self.body.is_none()
    }
}

impl hyper::body::Body for DeliveredBody {
    type Data = bytes::Bytes;
    type Error = Error;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        let this = &mut *self;
        if this.take_cut() {
            return std::task::Poll::Ready(None);
        }
        let Some(inner) = this.body.as_mut() else {
            return std::task::Poll::Ready(None);
        };
        let poll = std::pin::Pin::new(&mut *inner).poll_frame(cx);
        let ended = matches!(poll, std::task::Poll::Ready(None))
            || matches!(&poll, std::task::Poll::Ready(Some(Ok(frame))) if frame.is_trailers())
            || inner.is_end_stream();
        if matches!(poll, std::task::Poll::Ready(Some(Err(_)))) {
            this.body = None;
            return poll;
        }
        if poll.is_pending() {
            this.cut.waker.register(cx.waker());
            // A cut that landed before the registration found no waker.
            if this.take_cut() {
                return std::task::Poll::Ready(None);
            }
        }
        if ended {
            this.report(Ok(()));
        }
        poll
    }

    fn is_end_stream(&self) -> bool {
        self.body
            .as_ref()
            .is_none_or(hyper::body::Body::is_end_stream)
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.body.as_ref().map_or_else(
            || hyper::body::SizeHint::with_exact(0),
            hyper::body::Body::size_hint,
        )
    }
}

impl Drop for DeliveredBody {
    fn drop(&mut self) {
        let delivered = self.done.is_none()
            || self
                .body
                .as_ref()
                .is_some_and(|b| self.completion.is_complete(b));
        // Let the task report success; it bounds its own remaining request I/O.
        if delivered && let Some(task) = self.task.take() {
            task.detach();
        }
        self.report(if delivered {
            Ok(())
        } else {
            Err(Error::ConnectionTerminated)
        });
    }
}

/// Run the handler and the request's I/O together, giving the I/O at most
/// `bound` to finish once the handler is done: a delivered response must not
/// leave its store running on an upload nobody reads.
async fn join_bounding_io<H: Future>(
    handler: H,
    io: impl Future<Output = ()>,
    bound: std::time::Duration,
) -> H::Output {
    let handler = std::pin::pin!(handler);
    let io = std::pin::pin!(io);
    match futures::future::select(handler, io).await {
        futures::future::Either::Left((output, io)) => {
            if tokio::time::timeout(bound, io).await.is_err() {
                tracing::warn!("P3 request I/O outlived its response; abandoning it");
            }
            output
        }
        futures::future::Either::Right(((), handler)) => handler.await,
    }
}

impl Delivery {
    /// Resolves once the body was read whole (`Ok`) or dropped first.
    pub(crate) async fn ended(&mut self) -> Result<(), Error> {
        let delivery = (&mut self.done)
            .await
            .unwrap_or(Err(Error::ConnectionTerminated));
        self.ended = true;
        delivery
    }
}

impl Drop for Delivery {
    fn drop(&mut self) {
        if !self.ended {
            self.cut.requested.store(true, Ordering::Release);
            self.cut.waker.wake();
        }
    }
}

/// Handle an HTTP request using the WASIP3 `wasi:http/handler` interface.
///
/// P3 uses `ServicePre`/`Service` with `Store::run_concurrent` to get an
/// `Accessor` for concurrent component-model async operations.
///
/// The response body is **streamed**: the component runs in a background task
/// that owns the store and drives `run_concurrent`, and the server reads the
/// guest's body as the store produces it (see [`DeliveredBody`]). The store
/// and therefore the guest stays alive until the body has been fully drained,
/// and a slow client applies backpressure to the guest rather than buffering
/// the whole body in memory.
pub(crate) async fn handle_component_request_p3(
    mut warm: ComponentInstance,
    req: hyper::Request<WasiBody>,
    abandoned: std::sync::Arc<crate::engine::abandon::AbandonFlag>,
) -> anyhow::Result<hyper::Response<WasiBody>> {
    let completion = ResponseBodyCompletion::for_request(&req);
    // Named from the store this call was given, the same way `HttpTask` does:
    // the dispatcher does not have to know whose execution it is.
    let attributes =
        crate::host::http::stored_http_attributes(&warm.store.data().executed, req.method());
    // The same store's meter as the attributes, carried in because the closure
    // below is handed an accessor rather than the store.
    let executed = std::sync::Arc::clone(&warm.store.data().executed);
    let (wasi_req, req_io) = crate::host::http::p3_request(warm.store.data_mut(), req);

    // Delivers the response as soon as the handler returns it, while the body
    // is still streaming.
    let (parts_tx, parts_rx) =
        tokio::sync::oneshot::channel::<anyhow::Result<hyper::Response<DeliveredBody>>>();

    // The store is owned by this task for the entire response lifetime.
    // `run_concurrent` only resolves once the body has been delivered, so the
    // guest keeps running until the body is fully drained. Wrapped in an
    // `AbortOnDropHandle` immediately (not just once the body holds it) so that
    // if this request future is cancelled while still awaiting the response head
    // — e.g. the client disconnects before the guest produces a response — the
    // task is aborted rather than detached to run unbounded.
    let task = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
        async move {
            let ComponentInstance {
                mut store,
                instance,
            } = warm;
            // Watched for the life of the store's run: the dispatcher enforces
            // the deadline out where a non-yielding guest cannot block it.
            let _abandoned = store.data().abandoned.watch(abandoned);
            // A binding view over the instance, rebuilt per request. Cheap
            // (export lookups); the expensive part -- the store and the
            // instantiation -- is what a warm instance carries over.
            let service = match Service::new(&mut store, &instance) {
                Ok(service) => service,
                Err(e) => {
                    let _ = parts_tx.send(Err(
                        anyhow::anyhow!(e).context("failed to bind P3 service exports")
                    ));
                    return;
                }
            };

            let run = store
                .run_concurrent(async move |store| {
                    // The requests that reach this path are the ones a full
                    // pool turned away; leaving them out would empty the
                    // histogram exactly when the host is overloaded.
                    let _sample = crate::engine::instance_driver::InvocationSample::start(
                        &executed, attributes,
                    );
                    let mut parts_tx = Some(parts_tx);
                    let handler_fut = async move {
                        let response = match service.handle(store, wasi_req).await {
                            Ok(Ok(response)) => response,
                            Ok(Err(error_code)) => {
                                tracing::error!(?error_code, "P3 HTTP handler returned error");
                                if let Some(tx) = parts_tx.take() {
                                    let mut resp = hyper::Response::new(DeliveredBody::empty());
                                    *resp.status_mut() = hyper::StatusCode::INTERNAL_SERVER_ERROR;
                                    let _ = tx.send(Ok(resp));
                                }
                                return Ok(());
                            }
                            Err(e) => {
                                store.with(|mut access| {
                                    crate::engine::trap_remediation::note_guest_fault(
                                        access.data_mut(),
                                        &e,
                                    )
                                });
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
                            tokio::sync::oneshot::channel::<Result<(), Error>>();
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
                        let (head, body) = http_response.into_parts();
                        let (body, mut delivery) = DeliveredBody::new(body, completion);
                        // Wait for the body only once it is the server's: a
                        // caller that dropped the receiver dropped the body too.
                        if let Some(tx) = parts_tx.take() {
                            let _ = tx.send(Ok(hyper::Response::from_parts(head, body)));
                        }
                        let _ = finish_tx.send(delivery.ended().await);
                        Ok::<(), anyhow::Error>(())
                    };
                    let io_fut = async {
                        if let Err(e) = req_io.await {
                            tracing::error!(err = ?e, "P3 request I/O error");
                        }
                    };
                    join_bounding_io(handler_fut, io_fut, crate::timeouts::http_response()).await
                })
                .await;
            // This store served exactly this request: a component that keeps
            // instances warm is served by a driver instead (see
            // `engine::instance_driver`), and only reaches here when every warm
            // instance was busy.
            match run {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::error!(err = ?e, "P3 response streaming failed"),
                Err(e) => {
                    tracing::error!(err = ?e, "P3 run_concurrent failed");
                    crate::engine::trap_remediation::note_guest_fault(store.data(), &e);
                }
            }
            drop(store);
        }
        .in_current_span(),
    ));

    let resp = parts_rx
        .await
        .map_err(|_| anyhow::anyhow!("P3 component task ended before producing a response"))??;
    Ok(resp.map(|mut body| {
        body.task = Some(task);
        body.boxed_unsync()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use hyper::body::{Body as _, Frame};

    /// A body that never yields, reporting `remaining` bytes still to come.
    struct Stalled {
        remaining: u64,
    }

    impl hyper::body::Body for Stalled {
        type Data = Bytes;
        type Error = Error;

        fn poll_frame(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            std::task::Poll::Pending
        }

        fn size_hint(&self) -> hyper::body::SizeHint {
            hyper::body::SizeHint::with_exact(self.remaining)
        }
    }

    fn full(data: &'static [u8]) -> WasiBody {
        http_body_util::Full::new(Bytes::from_static(data))
            .map_err(|never| match never {})
            .boxed_unsync()
    }

    /// Reading the body whole reports a delivered response.
    #[tokio::test]
    async fn delivered_body_reports_a_whole_read() {
        let (mut body, mut delivery) =
            DeliveredBody::new(full(b"data"), ResponseBodyCompletion::Stream);
        let frame = body.frame().await.expect("a frame").expect("ok frame");
        assert_eq!(frame.into_data().expect("data").as_ref(), b"data");
        assert!(body.is_end_stream());
        assert!(delivery.ended().await.is_ok());
    }

    /// hyper drops a fixed-length body once its last byte is written, without
    /// reading to the end; with none of its length left, that is delivered.
    #[tokio::test]
    async fn delivered_body_counts_a_spent_length_as_delivered() {
        let (body, mut delivery) = DeliveredBody::new(
            Stalled { remaining: 0 }.boxed_unsync(),
            ResponseBodyCompletion::Http1,
        );
        drop(body);
        assert!(delivery.ended().await.is_ok());
    }

    #[tokio::test]
    async fn pending_trailers_dropped_on_a_stream_report_termination() {
        let (body, mut delivery) = DeliveredBody::new(
            Stalled { remaining: 0 }.boxed_unsync(),
            ResponseBodyCompletion::Stream,
        );
        drop(body);
        assert!(matches!(
            delivery.ended().await,
            Err(Error::ConnectionTerminated)
        ));
    }

    #[tokio::test]
    async fn streamed_trailers_are_delivered_before_success() {
        let inner = full(b"data")
            .with_trailers(std::future::ready(Some(Ok(hyper::HeaderMap::new()))))
            .boxed_unsync();
        let (mut body, mut delivery) = DeliveredBody::new(inner, ResponseBodyCompletion::Stream);
        assert!(body.frame().await.expect("data frame").is_ok());
        assert!(matches!(
            delivery.done.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        let frame = body.frame().await.expect("trailers").expect("valid frame");
        assert!(frame.is_trailers());
        drop(body);
        assert!(delivery.ended().await.is_ok());
    }

    #[tokio::test]
    async fn a_terminal_trailer_error_reports_failed_delivery() {
        let inner = full(b"data")
            .with_trailers(std::future::ready(Some(Err(Error::ConnectionTerminated))))
            .boxed_unsync();
        let (mut body, mut delivery) = DeliveredBody::new(inner, ResponseBodyCompletion::Stream);
        assert!(body.frame().await.expect("data frame").is_ok());
        assert!(body.frame().await.expect("error frame").is_err());
        drop(body);
        assert!(matches!(
            delivery.ended().await,
            Err(Error::ConnectionTerminated)
        ));
    }

    /// Dropping the body with bytes still to come reports the delivery failed.
    #[tokio::test]
    async fn delivered_body_dropped_early_reports_termination() {
        let (body, mut delivery) = DeliveredBody::new(
            Stalled { remaining: 4 }.boxed_unsync(),
            ResponseBodyCompletion::Stream,
        );
        drop(body);
        assert!(matches!(
            delivery.ended().await,
            Err(Error::ConnectionTerminated)
        ));
    }

    /// Dropping the [`Delivery`] cuts the response short: the server sees the
    /// body end, and the guest's stream loses its reader.
    #[tokio::test]
    async fn dropping_the_delivery_cuts_the_body() {
        let (mut body, delivery) = DeliveredBody::new(
            Stalled { remaining: 4 }.boxed_unsync(),
            ResponseBodyCompletion::Stream,
        );
        drop(delivery);
        assert!(body.frame().await.is_none());
        assert!(body.is_end_stream());
    }

    /// A cut wakes a server parked on a pending body, whose next poll ends it.
    #[test]
    fn a_cut_wakes_a_parked_server() {
        #[derive(Default)]
        struct Woken(AtomicBool);
        impl std::task::Wake for Woken {
            fn wake(self: Arc<Self>) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let (mut body, delivery) = DeliveredBody::new(
            Stalled { remaining: 4 }.boxed_unsync(),
            ResponseBodyCompletion::Stream,
        );
        let woken = Arc::new(Woken::default());
        let waker = std::task::Waker::from(Arc::clone(&woken));
        let mut cx = std::task::Context::from_waker(&waker);
        let mut poll = || std::pin::Pin::new(&mut body).poll_frame(&mut cx);

        assert!(poll().is_pending());
        drop(delivery);
        assert!(
            woken.0.load(Ordering::SeqCst),
            "the cut should wake the server"
        );
        assert!(matches!(poll(), std::task::Poll::Ready(None)));
    }

    /// A body read whole leaves the component task running, so it can tell
    /// the guest delivery succeeded and finish on its own.
    #[tokio::test]
    async fn delivered_body_drop_leaves_task_running() {
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = release_rx.await;
            let _ = finished_tx.send(());
        });

        let (mut body, _delivery) =
            DeliveredBody::new(full(b"data"), ResponseBodyCompletion::Stream);
        body.task = Some(tokio_util::task::AbortOnDropHandle::new(task));
        let _ = body.frame().await;
        drop(body);

        release_tx.send(()).expect("task still running");
        finished_rx
            .await
            .expect("the task should finish, not be aborted");
    }

    /// Request I/O still pending once the handler is done gets `bound`, not
    /// forever.
    #[tokio::test]
    async fn stalled_request_io_is_bounded_after_the_handler() {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            join_bounding_io(
                async { 7 },
                std::future::pending::<()>(),
                std::time::Duration::ZERO,
            ),
        )
        .await
        .expect("stalled I/O should not hold the handler's result");
        assert_eq!(output, 7);
    }

    /// I/O finishing first leaves the handler unbounded.
    #[tokio::test(start_paused = true)]
    async fn finished_request_io_leaves_the_handler_unbounded() {
        let handler = async {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            7
        };
        let output = join_bounding_io(handler, async {}, std::time::Duration::ZERO).await;
        assert_eq!(output, 7);
    }

    /// Dropping a `DeliveredBody` must abort the component task (via the held
    /// `AbortOnDropHandle`) so the guest is cancelled promptly on disconnect.
    #[tokio::test]
    async fn delivered_body_drop_aborts_task() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let (gone_tx, gone_rx) = tokio::sync::oneshot::channel::<()>();

        let task = tokio::spawn(async move {
            let _gone_tx = gone_tx; // dropped (closing the channel) only when the task ends
            let _ = started_tx.send(());
            // Runs "forever" unless aborted.
            std::future::pending::<()>().await;
        });

        let (mut body, _delivery) = DeliveredBody::new(
            Stalled { remaining: 4 }.boxed_unsync(),
            ResponseBodyCompletion::Stream,
        );
        body.task = Some(tokio_util::task::AbortOnDropHandle::new(task));
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
