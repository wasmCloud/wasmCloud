//! Host component plugin that CALLS its workloads rather than only serving
//! them: it imports `acme:events/handler`, which no host built-in provides, so
//! the host routes each call to the export of a bound workload.
//!
//! The three exported ops cover the two ways a call finds its workload:
//! `echo` names none and inherits the caller, `dispatch` names one with a
//! `wasmcloud:host/workload` target handle, and `nested` shows the handle's
//! lifetime is the scope by calling once inside it and once after.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "events-plugin", generate_all });
}

use bindings::acme::events::handler;
use bindings::exports::acme::events::control::Guest;
use bindings::wasmcloud::host::workload::{self, Target};

struct EventsPlugin;

impl Guest for EventsPlugin {
    /// No target handle is held, so the host sends this to the workload whose
    /// `control` call is running — the plugin calling back into its own caller.
    async fn echo(message: String) -> String {
        handler::notify(format!("echo:{message}")).await
    }

    /// The handle names the workload for as long as it is alive, so this
    /// reaches `id` even though the calling workload is someone else.
    async fn dispatch(id: String, message: String) -> String {
        let _target = Target::new(&id);
        handler::notify(format!("dispatch:{message}")).await
    }

    /// Held only for the inner block, so the second call falls back to the
    /// caller — the difference a scoped handle is supposed to make.
    async fn nested(id: String, message: String) -> String {
        let targeted = {
            let _target = Target::new(&id);
            handler::notify(format!("inner:{message}")).await
        };
        let untargeted = handler::notify(format!("outer:{message}")).await;
        format!("{targeted}|{untargeted}")
    }

    async fn callable() -> Vec<String> {
        workload::callable()
    }
}

bindings::export!(EventsPlugin with_types_in bindings);
