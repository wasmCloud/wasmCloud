//! The [`Ingress::Messaging`] path: `wasmcloud:messaging/handler@0.3.0`
//! invocations served on the shared service instance.
//!
//! Trigger services are p3-only, so this binds the async `@0.3.0` handler
//! through generated typed bindings — `AsyncMessaging::new` over the live
//! instance, the same shape `Service::new` gives the HTTP ingress — rather than
//! the hand-rolled `Val` lowering the p2 handler once required. Typed bindings
//! are also what make the `stream<u8>` message body workable: the delivered
//! bytes are minted into a native stream per invocation, which `Val` has no
//! ergonomic spelling for.
//!
//! [`Ingress::Messaging`]: super::Ingress::Messaging

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

pub(super) use bindings::AsyncMessaging;
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
pub struct MessagingJob {
    pub msg: BrokerMessage,
    pub result_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
    pub abandoned: Arc<AbandonFlag>,
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

/// Handles one inbound message on the shared service instance by invoking the
/// async `@0.3.0` `handle-message` export through its typed bindings, and
/// reports its disposition.
///
/// A handler `err` is an ordinary application outcome, reported on `result_tx`
/// only. A guest *trap*, however, leaves the shared instance unenterable for
/// every later message, so after reporting it the task returns the error —
/// faulting `run_concurrent` so the driver exits and the service supervisor
/// restarts (and re-registers) a fresh instance.
pub(super) struct MessagingTask {
    pub(super) handler: std::sync::Arc<AsyncMessaging>,
    pub(super) msg: BrokerMessage,
    pub(super) result_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
    pub(super) abandoned: Arc<AbandonFlag>,
}

impl AccessorTask<SharedCtx> for MessagingTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        let MessagingTask {
            handler,
            msg,
            result_tx,
            abandoned,
        } = self;

        // Watch this call for the rest of its life. The guard deregisters on
        // drop, however the call ends.
        let _abandoned = accessor.with(|mut access| {
            let calls = Arc::clone(&access.get().abandoned);
            calls.watch(abandoned)
        });

        let outcome = async {
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
        }
        .await;

        match outcome {
            Ok(result) => {
                let _ = result_tx.send(result.map_err(render_handle_error));
                Ok(())
            }
            Err(e) => {
                let _ = result_tx.send(Err(format!("handle-message trapped: {e:#}")));
                Err(e.context("messaging handler trapped; restarting the trigger service"))
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
