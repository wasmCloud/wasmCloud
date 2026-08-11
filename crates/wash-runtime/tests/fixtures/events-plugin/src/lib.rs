//! Host component plugin that CALLS its workloads rather than only serving
//! them: it imports `acme:events/handler`, which no host built-in provides, so
//! the host routes each call to the export of a bound workload.
//!
//! The exported ops cover the two ways a call finds its workload: `echo` names
//! none and inherits the caller, `dispatch` names one with a
//! `wasmcloud:host/workload-call` target handle, and `nested` shows the
//! handle's lifetime is the scope by calling once inside it and once after.
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
use bindings::wasmcloud::host::types::CallError;
use bindings::exports::acme::events::control::Guest;
use bindings::exports::wasmcloud::host::workload_lifecycle::{
    Guest as LifecycleGuest, WorkloadInfo,
};
use bindings::wasmcloud::host::workload_call::{self, Target};

/// Workload id -> what each lifecycle hook saw when it tried to open a target
/// for that workload. Instance memory, so it is empty again after a restart.
static PROBE: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// The interfaces this plugin imports for a workload to export, spelled as
/// `callable` reports them. A `target` is opened for one of these, not for a
/// workload alone — the pairing is what makes a handle proof the call routes.
const HANDLER: &str = "acme:events/handler@0.1.0";
const METRICS: &str = "acme:events/metrics@0.1.0";

/// Try to open a target for `id` from inside a hook and append what happened.
fn probe(id: &str, phase: &str) {
    let seen = match Target::open(id, HANDLER) {
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

/// Render a failed call the way a plugin would log it, so a test can read which
/// case the host chose. Every one of these is a value the plugin *received* —
/// none of them trapped it.
fn failed(err: CallError) -> String {
    let (case, detail) = match err {
        CallError::NoTarget(detail) => ("no-target", detail),
        CallError::NotRunning(detail) => ("not-running", detail),
        CallError::NotExported(detail) => ("not-exported", detail),
        CallError::Failed(detail) => ("failed", detail),
        // Matched because the host may grow into it; the set is not closed.
        CallError::Other(detail) => ("other", detail),
    };
    let _ = detail;
    format!("error:{case}")
}

/// Render a `metrics` failure. The case is this interface's own, so the host
/// picked the nearest thing it had — which is why the detail, where a case has
/// room for one, is prefixed to say the host produced it.
fn metrics_failure(err: metrics::MetricsError) -> String {
    match err {
        metrics::MetricsError::Rejected(_) => "metrics-error:rejected".to_string(),
        metrics::MetricsError::InternalError(_) => "metrics-error:internal-error".to_string(),
    }
}

/// Call `handler.notify` and flatten either outcome into one string.
async fn notify(message: String) -> String {
    match handler::notify(message).await {
        Ok(reply) => reply,
        Err(err) => failed(err),
    }
}

impl Guest for EventsPlugin {
    /// No target handle is held, so the host sends this to the workload whose
    /// `control` call is running — the plugin calling back into its own caller.
    async fn echo(message: String) -> String {
        notify(format!("echo:{message}")).await
    }

    /// The handle names the workload for as long as it is alive, so this
    /// reaches `id` even though the calling workload is someone else. `open`
    /// answering `none` is the ordinary way a dispatch loop learns a workload
    /// went away, and a call it did make coming back `Err` is the other — both
    /// are values, neither takes the plugin down.
    async fn dispatch(id: String, message: String) -> String {
        let Some(target) = Target::open(&id, HANDLER) else {
            return format!("unroutable:{id}");
        };
        // Read the handle back rather than reuse `id`, so the reply carries the
        // workload the *host* thinks this call is scoped to.
        let named = target.get_workload_id();
        notify(format!("dispatch:{named}:{message}")).await
    }

    /// Held only for the inner block, so the second call falls back to the
    /// caller — the difference a scoped handle is supposed to make.
    async fn nested(id: String, message: String) -> String {
        let targeted = match Target::open(&id, HANDLER) {
            Some(_target) => notify(format!("inner:{message}")).await,
            None => format!("unroutable:{id}"),
        };
        let untargeted = notify(format!("outer:{message}")).await;
        format!("{targeted}|{untargeted}")
    }

    /// Flatten the map the host hands back so an HTTP test can read it. The
    /// interfaces are what make it worth a map: this plugin imports two, and a
    /// workload exporting only one must be reported that way.
    async fn callable() -> Vec<String> {
        workload_call::callable()
            .into_iter()
            .map(|(id, interfaces)| format!("{id}={}", interfaces.join(",")))
            .collect()
    }

    /// Calls `metrics` on a workload that does not export it, which the host
    /// answers with `not-exported` rather than a trap. Also what makes the
    /// plugin genuinely import a second workload-facing interface.
    async fn report(id: String, measurement: String) -> String {
        let Some(_target) = Target::open(&id, METRICS) else {
            // No workload exports `metrics`, so this is where a plugin finds
            // out — before the call, not from its failure.
            return format!("unroutable:{id}");
        };
        match metrics::report(measurement).await {
            Ok(reply) => reply,
            Err(err) => metrics_failure(err),
        }
    }

    /// No handle, so this inherits the caller — which does not export `metrics`.
    /// The host reports that through `metrics`'s own error type, having no case
    /// of its own to use.
    async fn report_inherited(measurement: String) -> String {
        match metrics::report(measurement).await {
            Ok(reply) => reply,
            Err(err) => metrics_failure(err),
        }
    }

    async fn lifecycle_probe(id: String) -> String {
        PROBE.lock().unwrap().get(&id).cloned().unwrap_or_default()
    }
}

bindings::export!(EventsPlugin with_types_in bindings);
