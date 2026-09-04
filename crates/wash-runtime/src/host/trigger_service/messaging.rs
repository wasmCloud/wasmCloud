//! `wasmcloud:messaging/handler@0.3.0` invocations, on a shared service
//! instance or on a pooled one.
//!
//! Both shapes bind the async `@0.3.0` handler through generated typed
//! bindings — `AsyncMessaging::new` over the live instance, the same shape
//! `Service::new` gives the HTTP ingress — rather than the hand-rolled `Val`
//! lowering the p2 handler once required. Typed bindings are also what make the
//! `stream<u8>` message body workable: the delivered bytes are minted into a
//! native stream per invocation, which `Val` has no ergonomic spelling for.
//!
//! [`MessagingJob`] serves both, exactly as [`ServiceHttpJob`] does for HTTP:
//! the [`Ingress::Messaging`] path delivers to a service's singleton instance,
//! and [`InstanceJob::Messaging`] delivers to whichever pooled instance is
//! free. What separates them is [`MessagingTask::pool_slot`] — a service's
//! instance is not the pool's to retire.
//!
//! Only `@0.3.0` reaches either. `handle-message` is an `async func` there, so
//! its call takes an [`Accessor`] and several deliveries can share one instance
//! up to `max_concurrency`. The sync `@0.2.0` export takes `&mut Store` for the
//! length of its call, so it cannot share an instance at all and keeps its
//! per-message store.
//!
//! [`Ingress::Messaging`]: super::Ingress::Messaging
//! [`ServiceHttpJob`]: crate::host::http::ServiceHttpJob
//! [`InstanceJob::Messaging`]: crate::engine::instance_driver::InstanceJob::Messaging

use std::sync::Arc;

use wasmtime::component::{Accessor, AccessorTask, Instance, StreamReader};

use crate::engine::abandon::AbandonFlag;
use crate::engine::ctx::SharedCtx;

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "async-messaging",
        imports: { default: async | trappable | tracing },
        exports: { default: async | tracing },
    });
}

pub(crate) use bindings::AsyncMessaging;
use bindings::wasmcloud::messaging0_3_0::types::{
    BrokerMessage as WitBrokerMessage, HandleMessageError,
};

// The disposition strings a delivery reports are shared with the standalone
// plugins' per-message path, so they come from one place.
crate::plugin::wasmcloud_messaging::messaging_disposition_rendering! {
    disposition: HandleMessageError,
}

/// An inbound message delivered to the service's `wasmcloud:messaging/handler`
/// export. Bytes rather than a stream: the host-side ingress receives a bounded
/// payload from its backend, and the per-invocation task mints the `stream<u8>`
/// the `@0.3.0` WIT carries.
pub struct BrokerMessage {
    pub subject: String,
    pub body: Vec<u8>,
    pub reply_to: Option<String>,
}

/// A messaging invocation: the message, a oneshot carrying the handler's
/// outcome back to the host-side ingress (to ack/log, its disposition rendered
/// to a string), and the abandonment flag of the dispatched call enforcing its
/// deadline (see [`crate::engine::abandon`]).
///
/// Handle-free by construction — the body is bytes, and the `stream<u8>` the
/// WIT carries is minted inside the invocation once the store is chosen — which
/// is what lets the same job cross a channel to a service or go into the
/// instance pool.
pub struct MessagingJob {
    pub msg: BrokerMessage,
    pub result_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
    pub abandoned: Arc<AbandonFlag>,
    /// What this call is measured under. Built by the dispatcher, which is the
    /// layer that knows the workload's manifest identity — see
    /// [`crate::observability::WorkloadIdentity`].
    pub attributes: std::sync::Arc<[opentelemetry::KeyValue]>,
}

/// The messaging handler export a trigger service must provide.
///
/// Trigger services are p3-only by design, so this resolves `@0.3.0` and nothing
/// else. A component exporting only the sync `@0.2.0` handler is still served —
/// but as a per-message component, through the standalone NATS/in-memory plugins
/// rather than here.
///
/// Deliberately a single revision rather than a newest-first list: a component
/// may legally export *both* revisions, and picking one from a list would
/// silently ignore the other. Resolving exactly one makes the choice explicit
/// and the failure mode legible.
pub(super) const MESSAGING_HANDLER: &str = "wasmcloud:messaging/handler@0.3.0";

/// The sync handler revision, which the trigger service never invokes. Named
/// here only to detect (and warn about) a component exporting both revisions.
const SYNC_MESSAGING_HANDLER: &str = "wasmcloud:messaging/handler@0.2.0";

/// Bind the typed `@0.3.0` messaging view over the shared service instance.
/// `AsyncMessaging::new` is itself the export type-check, so a service missing
/// the export (or exporting only `@0.2.0`) fails here, before any message is
/// accepted for delivery.
///
/// A dual export (both revisions at once) warns rather than errors: the
/// workload is fully serviceable through `@0.3.0`, just not through the export
/// its author may have expected.
pub(super) fn bind_handler(
    store: &mut wasmtime::Store<SharedCtx>,
    instance: &Instance,
) -> wasmtime::Result<AsyncMessaging> {
    if instance
        .get_export(&mut *store, None, SYNC_MESSAGING_HANDLER)
        .is_some()
        && instance
            .get_export(&mut *store, None, MESSAGING_HANDLER)
            .is_some()
    {
        tracing::warn!(
            served = MESSAGING_HANDLER,
            ignored = SYNC_MESSAGING_HANDLER,
            "service exports both messaging handler revisions; the trigger service is \
             p3-only, so only the @0.3.0 handler will be invoked — export exactly one \
             messaging handler revision"
        );
    }
    AsyncMessaging::new(&mut *store, instance).map_err(|e| {
        e.context(format!(
            "service is missing the {MESSAGING_HANDLER} export (trigger services are \
             p3-only; a sync @0.2.0 handler must run as a per-message component)"
        ))
    })
}

/// Handles one inbound message by invoking the async `@0.3.0` `handle-message`
/// export through its typed bindings, and reports its disposition.
///
/// A handler `err` is an ordinary application outcome, reported on `result_tx`
/// only. A guest *trap*, however, leaves the instance unenterable for every
/// later message, so after reporting it the task returns the error — faulting
/// `run_concurrent` so the driver exits. A service's supervisor then restarts
/// and re-registers a fresh instance; a pooled instance's handle is reaped by
/// the next `offer`, and the delivery after it starts a new one.
pub(crate) struct MessagingTask {
    pub(crate) handler: std::sync::Arc<AsyncMessaging>,
    pub(crate) msg: BrokerMessage,
    pub(crate) result_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
    pub(crate) abandoned: Arc<AbandonFlag>,
    /// What this call is measured under; see
    /// [`crate::observability::WorkloadIdentity`].
    pub(crate) attributes: std::sync::Arc<[opentelemetry::KeyValue]>,
    /// This delivery's tether to a pooled instance: holds its in-flight slot
    /// and can retire the instance. `None` for the two shapes with no instance
    /// to retire — a service, whose singleton is not the pool's, and a cold
    /// store serving one delivery and being dropped.
    pub(crate) pool_slot: Option<crate::engine::instance_driver::PoolSlot>,
}

impl AccessorTask<SharedCtx> for MessagingTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        let MessagingTask {
            handler,
            msg,
            result_tx,
            abandoned,
            attributes,
            pool_slot,
        } = self;

        // The epoch deadline measures this call's own execution, so re-arm it
        // here. `watch_until_abandoned` below owns the registration.
        let (calls, executed) = accessor.with(|mut access| {
            crate::engine::abandon::rearm_for_call(&mut access);
            (
                Arc::clone(&access.get().abandoned),
                Arc::clone(&access.get().executed),
            )
        });
        // Both delivery shapes — a pooled instance and a long-lived service —
        // land here, so one sample covers both.
        let _sample =
            crate::engine::instance_driver::InvocationSample::start(&executed, attributes);

        let deliver = async {
            // The `@0.3.0` body is a native `stream<u8>`; mint one carrying the
            // delivered bytes for the guest to drain.
            let body = accessor.with(|mut a| StreamReader::new(&mut a, msg.body))?;
            let wit_msg = WitBrokerMessage {
                subject: msg.subject,
                body,
                reply_to: msg.reply_to,
            };
            handler
                .wasmcloud_messaging0_3_0_handler()
                .call_handle_message(accessor, wit_msg)
                .await
        };

        // `watch_until_abandoned` bounds only how long an overrunning delivery
        // stays visible to the epoch callback, whose trap would take every
        // other delivery sharing this instance. The deadline the *caller* waits
        // out is enforced by its `DispatchedCall`, outside this store.
        let watched = crate::engine::abandon::watch_until_abandoned(&calls, abandoned, deliver);

        // On a *pooled* instance the delivery is additionally bounded here, for
        // the guest work the host cannot cancel: once the dispatcher has given
        // up, retirement is what ends it — stop admitting, drain, and let the
        // store's teardown take the stalled work with it.
        //
        // Without a slot there is nothing to retire, so the arm is left
        // unbounded. For a cold store the dispatcher dropping this future is
        // already the remedy. For a service it is load-bearing: a wedged guest
        // is ended by the epoch callback trapping the store, which faults
        // `run_concurrent` and restarts the service, and reporting a timeout
        // first would return `Ok` and keep the wedged delivery on the instance
        // every other subject shares.
        //
        // TODO: both arms want per-task cancellation
        // (bytecodealliance/wasmtime#11833).
        let outcome = match pool_slot {
            None => watched.await,
            Some(slot) => {
                match tokio::time::timeout(crate::timeouts::messaging_deliver(), watched).await {
                    Ok(outcome) => outcome,
                    Err(_) => {
                        let _ = result_tx.send(Err("handle-message timed out".to_string()));
                        slot.retire_instance();
                        tracing::error!(
                            "messaging delivery timed out; retiring its pooled instance to \
                             end the stalled work"
                        );
                        return Ok(());
                    }
                }
            }
        };

        match outcome {
            Ok(result) => {
                let _ = result_tx.send(result.map_err(render_handle_error));
                Ok(())
            }
            Err(e) => {
                let _ = result_tx.send(Err(format!("handle-message trapped: {e:#}")));
                Err(e.context("messaging handler trapped; discarding its instance"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Payload-less dispositions render as the bare case name; `other` is the
    /// one case carrying detail, and it must survive.
    #[test]
    fn renders_dispositions() {
        assert_eq!(render_handle_error(HandleMessageError::Reject), "reject");
        assert_eq!(render_handle_error(HandleMessageError::Retry), "retry");
        assert_eq!(
            render_handle_error(HandleMessageError::Other("no responders".into())),
            "other: no responders"
        );
    }
}
