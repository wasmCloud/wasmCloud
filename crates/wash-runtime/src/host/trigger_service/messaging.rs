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

/// Interface names for the messaging handler export, newest first.
///
/// A service may export either revision: `@0.3.0`, whose `handle-message` is an
/// `async func` returning `result<_, error-variant>`, or the original `@0.2.0`
/// returning `result<_, string>`. Resolution prefers the async one and falls
/// back, so a component built before the async revision keeps working.
pub(super) const MESSAGING_HANDLERS: [&str; 2] = [
    "wasmcloud:messaging/handler@0.3.0",
    "wasmcloud:messaging/handler@0.2.0",
];
pub(super) const HANDLE_MESSAGE: &str = "handle-message";

/// Handles one inbound message on the shared service instance by invoking the
/// `handle-message` export via the dynamic concurrent path, and reports its
/// `result`.
///
/// `call_concurrent` drives an async-lifted (`@0.3.0`) export just as well as a
/// sync (`@0.2.0`) one, so one code path serves both revisions; only the error
/// payload differs, which [`lift_handler_result`] handles by shape.
///
/// A handler `Err(string)` is an ordinary application outcome, reported on
/// `result_tx` only. A guest *trap*, however, leaves the shared instance
/// unenterable for every later message, so after reporting it the task returns
/// the error — faulting `run_concurrent` so the driver exits and the service
/// supervisor restarts (and re-registers) a fresh instance.
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
            Ok(()) => lift_handler_result(results.first()),
            Err(e) => {
                let _ = result_tx.send(Err(format!("handle-message trapped: {e:#}")));
                return Err(e.context("messaging handler trapped; restarting the trigger service"));
            }
        };
        let _ = result_tx.send(outcome);
        Ok(())
    }
}

/// Lift the handler's `result` into a Rust `Result`, rendering the `err` payload
/// to a display string for the ack/log path.
///
/// Both handler revisions land here and are told apart by the payload's shape,
/// so neither the caller nor the ingress has to track which one the service
/// exports:
///
/// * `@0.2.0` — `result<_, string>`, so the payload is a [`Val::String`].
/// * `@0.3.0` — `result<_, error>`, so it is a [`Val::Variant`]. Payload-less
///   cases render as the case name (`timeout`); `other` renders as
///   `other: <detail>`, keeping the backend's message.
fn lift_handler_result(v: Option<&Val>) -> Result<(), String> {
    match v {
        Some(Val::Result(Ok(_))) => Ok(()),
        Some(Val::Result(Err(Some(boxed)))) => match &**boxed {
            Val::String(s) => Err(s.clone()),
            Val::Variant(case, None) => Err(case.clone()),
            Val::Variant(case, Some(payload)) => match &**payload {
                Val::String(s) => Err(format!("{case}: {s}")),
                other => Err(format!("{case}: {other:?}")),
            },
            other => Err(format!("{other:?}")),
        },
        Some(Val::Result(Err(None))) => Err(String::new()),
        other => Err(format!("unexpected result value: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifts_ok() {
        assert_eq!(lift_handler_result(Some(&Val::Result(Ok(None)))), Ok(()));
    }

    /// `@0.2.0` returns `result<_, string>`.
    #[test]
    fn lifts_v0_2_string_error() {
        let v = Val::Result(Err(Some(Box::new(Val::String("boom".into())))));
        assert_eq!(lift_handler_result(Some(&v)), Err("boom".to_string()));
    }

    /// `@0.3.0`'s payload-less cases render as the bare case name.
    #[test]
    fn lifts_v0_3_unit_variant_error() {
        let v = Val::Result(Err(Some(Box::new(Val::Variant("timeout".into(), None)))));
        assert_eq!(lift_handler_result(Some(&v)), Err("timeout".to_string()));
    }

    /// `other` is the one `@0.3.0` case carrying detail; it must survive.
    #[test]
    fn lifts_v0_3_other_variant_error_with_detail() {
        let v = Val::Result(Err(Some(Box::new(Val::Variant(
            "other".into(),
            Some(Box::new(Val::String("no responders".into()))),
        )))));
        assert_eq!(
            lift_handler_result(Some(&v)),
            Err("other: no responders".to_string())
        );
    }
}
