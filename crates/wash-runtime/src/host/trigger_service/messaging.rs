//! The [`Ingress::Messaging`] path: `wasmcloud:messaging/handler@0.2.0`
//! invocations served on the shared service instance.
//!
//! [`Ingress::Messaging`]: super::Ingress::Messaging

use wasmtime::component::{Accessor, AccessorTask, ComponentExportIndex, Instance, Val};

use crate::engine::ctx::SharedCtx;

/// An inbound message delivered to the service's `wasmcloud:messaging/handler`
/// export. Mirrors the `broker-message` record.
pub struct BrokerMessage {
    pub subject: String,
    pub body: Vec<u8>,
    pub reply_to: Option<String>,
}

/// A messaging invocation: the message plus a oneshot carrying the handler's
/// `result<_, string>` outcome back to the host-side ingress (to ack/log).
pub type MessagingJob = (
    BrokerMessage,
    tokio::sync::oneshot::Sender<Result<(), String>>,
);

/// Interface + function names for the messaging handler export.
pub(super) const MESSAGING_HANDLER: &str = "wasmcloud:messaging/handler@0.2.0";
pub(super) const HANDLE_MESSAGE: &str = "handle-message";

/// Handles one inbound message on the shared service instance by invoking the
/// p2 `handle-message` export via the dynamic concurrent path (there is no
/// accessor-driven p3 messaging binding), and reports its `result<_, string>`.
pub(super) struct MessagingTask {
    pub(super) instance: Instance,
    pub(super) func_idx: ComponentExportIndex,
    pub(super) msg: BrokerMessage,
    pub(super) result_tx: tokio::sync::oneshot::Sender<Result<(), String>>,
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
        let outcome = match func
            .call_concurrent(accessor, &[message], &mut results)
            .await
        {
            Ok(()) => lift_result_string(results.first()),
            Err(e) => Err(format!("handle-message trapped: {e:#}")),
        };
        let _ = result_tx.send(outcome);
        Ok(())
    }
}

/// Lift a `result<_, string>` value into a Rust `Result`, mapping the `err` case
/// to its string (empty when the payload is absent).
fn lift_result_string(v: Option<&Val>) -> Result<(), String> {
    match v {
        Some(Val::Result(Ok(_))) => Ok(()),
        Some(Val::Result(Err(Some(boxed)))) => match &**boxed {
            Val::String(s) => Err(s.clone()),
            other => Err(format!("{other:?}")),
        },
        Some(Val::Result(Err(None))) => Err(String::new()),
        other => Err(format!("unexpected result value: {other:?}")),
    }
}
