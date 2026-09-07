//! Host component plugin that runs its workloads: it imports `wasi:cli/run`,
//! which no host built-in provides, so the host routes each call to the
//! component of a bound workload that exports it.
//!
//! `control` drives the two ways a call finds its workload — `run-inherited`
//! names none and goes back to the calling workload, `run-workload` names one
//! with a `wasmcloud:host/workload-call` target handle. `dispatched` reports
//! what the plugin's OWN exported `wasi:cli/run` loop has done: a trigger
//! driving workloads off its own clock, with no inbound call at all.
//!
//! `run` returns a bare `result`, so a failed call carries no reason — the host
//! still answers with a value rather than trapping the plugin.

mod bindings;

use std::collections::BTreeMap;
use std::sync::Mutex;

use bindings::exports::acme::clirun::control::Guest;
use bindings::exports::wasi::cli::run::Guest as RunGuest;
use bindings::wasi::clocks::monotonic_clock;
use bindings::wasmcloud::host::workload_call::{self, Target};

/// The interfaces this plugin imports for a workload to export. A `target` is
/// opened for one of these, not for a workload alone.
const RUN: &str = "wasi:cli/run@0.3.0";
const PROBE: &str = "acme:clirun/probe@0.1.0";

/// Workload id -> runs the plugin's own `cli/run` loop has dispatched to it.
/// Instance memory, so it starts over if the plugin ever restarts.
static DISPATCHED: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

struct CliRunPlugin;

/// Call the imported `run` and flatten either outcome into one word.
async fn run() -> &'static str {
    match bindings::wasi::cli::run::run().await {
        Ok(()) => "ok",
        Err(()) => "err",
    }
}

impl Guest for CliRunPlugin {
    async fn run_workload(id: String) -> String {
        // `open` answering `none` is how a dispatch loop learns a workload went
        // away, or never exported `run`. Calling blind instead would trap the
        // store this plugin shares with every workload it serves.
        let Some(_target) = Target::open(&id, RUN) else {
            return format!("unroutable:{id}");
        };
        run().await.to_string()
    }

    async fn run_inherited() -> String {
        run().await.to_string()
    }

    async fn probe_workload(id: String) -> String {
        let Some(_target) = Target::open(&id, PROBE) else {
            return format!("unroutable:{id}");
        };
        // A failure arrives as `Err(())` — the error arm carries nothing, which
        // is all `result<T>` offers. It is still a value, not a trap.
        match bindings::acme::clirun::probe::check().await {
            Ok(value) => format!("ok:{value}"),
            Err(()) => "err".to_string(),
        }
    }

    async fn dispatched() -> Vec<String> {
        DISPATCHED
            .lock()
            .unwrap()
            .iter()
            .map(|(id, count)| format!("{id}={count}"))
            .collect()
    }
}

impl RunGuest for CliRunPlugin {
    /// The plugin's own long-running work, co-driven by the host. Every tick it
    /// runs each workload that exports `run`, which is what a cron-style
    /// trigger built as a component plugin does.
    async fn run() -> Result<(), ()> {
        loop {
            monotonic_clock::wait_for(20_000_000).await;
            let targets: Vec<String> = workload_call::callable()
                .into_iter()
                .filter(|(_, interfaces)| interfaces.iter().any(|i| i == RUN))
                .map(|(id, _)| id)
                .collect();
            for id in targets {
                // The workload can go away between `callable` and here, so the
                // handle is what decides whether the call is made at all.
                let Some(_target) = Target::open(&id, RUN) else {
                    continue;
                };
                if bindings::wasi::cli::run::run().await.is_ok() {
                    *DISPATCHED.lock().unwrap().entry(id).or_default() += 1;
                }
            }
        }
    }
}

bindings::export!(CliRunPlugin with_types_in bindings);
