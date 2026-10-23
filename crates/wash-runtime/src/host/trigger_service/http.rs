//! The [`Ingress::Http`] path: `wasi:http/handler@0.3` requests served on the
//! shared service instance, streaming responses back to the HTTP server.
//!
//! [`Ingress::Http`]: super::Ingress::Http

use std::sync::Arc;

use http_body_util::BodyExt;
use wasmtime::component::{Accessor, AccessorTask};
use wasmtime_wasi_http::p3::bindings::Service;
use wasmtime_wasi_http::{Error, WasiBody};

use crate::engine::ctx::SharedCtx;
use crate::host::http::ResponseBodyCompletion;
use crate::host::http_p3::DeliveredBody;

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
    pub(crate) req: hyper::Request<WasiBody>,
    pub(crate) resp_tx: tokio::sync::oneshot::Sender<anyhow::Result<hyper::Response<WasiBody>>>,
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
        // Named and recorded from the store this runs on, stamped with whose
        // execution it is when it was built: a service's ingress has no
        // workload handle to look either one up from. An unstamped store is a
        // host measuring nothing, and costs neither the attributes nor the
        // sample.
        let (calls, executed) = accessor.with(|mut access| {
            crate::engine::abandon::rearm_for_call(&mut access);
            (
                std::sync::Arc::clone(&access.get().abandoned),
                std::sync::Arc::clone(&access.get().executed),
            )
        });
        let attributes = crate::host::http::stored_http_attributes(&executed, req.method());
        let _sample =
            crate::engine::instance_driver::InvocationSample::start(&executed, attributes);

        let completion = ResponseBodyCompletion::for_request(&req);
        let (wasi_req, req_io) =
            accessor.with(|mut access| crate::host::http::p3_request(access.get(), req));

        let mut resp_tx = Some(resp_tx);

        let handler_fut = async move {
            let response = match service.handle(accessor, wasi_req).await {
                Ok(Ok(response)) => response,
                Ok(Err(error_code)) => {
                    tracing::error!(?error_code, "service HTTP handler returned error");
                    if let Some(tx) = resp_tx.take() {
                        let resp = hyper::Response::builder()
                            .status(500)
                            .body(WasiBody::default())
                            .map_err(anyhow::Error::from);
                        let _ = tx.send(resp);
                    }
                    return Ok(());
                }
                Err(e) => {
                    accessor.with(|mut access| {
                        crate::engine::trap_remediation::note_guest_fault(access.data_mut(), &e)
                    });
                    if let Some(tx) = resp_tx.take() {
                        let _ =
                            tx.send(Err(anyhow::anyhow!(e).context("service HTTP handler trap")));
                    }
                    return Ok(());
                }
            };

            // `into_http`'s future reports the body-delivery outcome back to the
            // guest; resolve it once the body has been fully forwarded.
            let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<Result<(), Error>>();
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
            let (head, body) = http_response.into_parts();
            let (body, mut delivery) = DeliveredBody::new(body, completion);

            // Deliver the head + streaming body to the HTTP server now. Wait for
            // the body only once it is the server's: a caller that dropped the
            // receiver dropped the body too.
            if let Some(tx) = resp_tx.take() {
                let _ = tx.send(Ok(hyper::Response::from_parts(head, body.boxed_unsync())));
            }
            let _ = finish_tx.send(delivery.ended().await);
            Ok::<(), anyhow::Error>(())
        };
        let io_fut = async move {
            let _ = req_io.await;
        };

        // Bound the whole exchange so a stalled client (connected but not reading)
        // can't park this task on its body's delivery for the life of the
        // connection. A response still streaming past this bound is truncated:
        // dropping its `Delivery` cuts the body.
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
