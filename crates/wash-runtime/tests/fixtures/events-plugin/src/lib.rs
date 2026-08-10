//! Host component plugin that CALLS its workloads rather than only serving
//! them: it imports `acme:events/handler`, which no host built-in provides, so
//! the host routes each call to the export of a bound workload.
//!
//! The exported ops cover the two ways a call finds its workload: `echo` names
//! none and inherits the caller, `dispatch` names one with a
//! `wasmcloud:host/workload` target handle, and `nested` shows the handle's
//! lifetime is the scope by calling once inside it and once after.
//!
//! It imports a *second* workload-facing interface, `acme:events/metrics`, that
//! no workload exports, so `callable` has to report per workload which of the
//! two is reachable rather than just naming the workload.
//!
//! It also exports the lifecycle hooks, purely to record what a hook can
//! reach. A workload is callable only while it is running, so neither hook can
//! call the workload it is about — `on-workload-bind` arrives while it is
//! still deploying and `on-workload-unbind` after it has been torn down. The
//! hooks check with `target.open` rather than calling blind, which is the
//! pattern that keeps a plugin from trapping: a workload-exported import has no
//! error result, so a call that cannot be routed takes the plugin's store down
//! with it.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "events-plugin", generate_all });
}

use std::collections::BTreeMap;
use std::sync::Mutex;

use bindings::acme::events::{handler, metrics};
use bindings::exports::acme::events::control::Guest;
use bindings::exports::wasmcloud::host::workload_lifecycle::{
    Guest as LifecycleGuest, WorkloadInfo,
};
use bindings::wasmcloud::host::workload::{self, Target};

/// Workload id -> what each lifecycle hook saw when it tried to open a target
/// for that workload. Instance memory, so it is empty again after a restart.
static PROBE: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// Try to open a target for `id` from inside a hook and append what happened.
fn probe(id: &str, phase: &str) {
    let seen = match Target::open(id) {
        Some(_) => "some",
        None => "none",
    };
    let mut probe = PROBE.lock().unwrap();
    let entry = probe.entry(id.to_string()).or_default();
    if !entry.is_empty() {
        entry.push('|');
    }
    entry.push_str(phase);
    entry.push(':');
    entry.push_str(seen);
}

struct EventsPlugin;

impl LifecycleGuest for EventsPlugin {
    async fn on_workload_bind(workload: WorkloadInfo) -> Result<(), String> {
        probe(&workload.id, "bind");
        Ok(())
    }

    async fn on_workload_unbind(id: String) {
        probe(&id, "unbind");
    }
}

impl Guest for EventsPlugin {
    /// No target handle is held, so the host sends this to the workload whose
    /// `control` call is running — the plugin calling back into its own caller.
    async fn echo(message: String) -> String {
        handler::notify(format!("echo:{message}")).await
    }

    /// The handle names the workload for as long as it is alive, so this
    /// reaches `id` even though the calling workload is someone else. `open`
    /// answering `none` is the ordinary way a dispatch loop learns a workload
    /// went away — the plugin reports it and carries on rather than calling
    /// into nothing and trapping.
    async fn dispatch(id: String, message: String) -> String {
        let Some(target) = Target::open(&id) else {
            return format!("unroutable:{id}");
        };
        // Read the handle back rather than reuse `id`, so the reply carries the
        // workload the *host* thinks this call is scoped to.
        let named = target.get_workload_id();
        handler::notify(format!("dispatch:{named}:{message}")).await
    }

    /// Held only for the inner block, so the second call falls back to the
    /// caller — the difference a scoped handle is supposed to make.
    async fn nested(id: String, message: String) -> String {
        let targeted = match Target::open(&id) {
            Some(_target) => handler::notify(format!("inner:{message}")).await,
            None => format!("unroutable:{id}"),
        };
        let untargeted = handler::notify(format!("outer:{message}")).await;
        format!("{targeted}|{untargeted}")
    }

    /// Flatten the map the host hands back so an HTTP test can read it. The
    /// interfaces are what make it worth a map: this plugin imports two, and a
    /// workload exporting only one must be reported that way.
    async fn callable() -> Vec<String> {
        workload::callable()
            .into_iter()
            .map(|(id, interfaces)| format!("{id}={}", interfaces.join(",")))
            .collect()
    }

    /// Never called by a test — calling `metrics` on a workload that does not
    /// export it traps. It exists so the plugin genuinely imports a second
    /// workload-facing interface.
    async fn report(id: String, measurement: String) -> String {
        let Some(_target) = Target::open(&id) else {
            return format!("unroutable:{id}");
        };
        metrics::report(measurement).await
    }

    async fn lifecycle_probe(id: String) -> String {
        PROBE.lock().unwrap().get(&id).cloned().unwrap_or_default()
    }
}

bindings::export!(EventsPlugin with_types_in bindings);
