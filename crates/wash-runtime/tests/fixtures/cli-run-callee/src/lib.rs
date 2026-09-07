//! A p3 component exporting `wasi:cli/run` and nothing else — what a service
//! or a host component plugin calls to run a workload.
//!
//! `RUN_MODE` picks how the run ends: returning either arm of `run`'s own
//! `result`, or leaving through `wasi:cli/exit`, which unwinds the instance
//! rather than returning. The run count is per instance, so it says whether the
//! host built a store for this call or reused a warm one.

mod bindings;

use std::sync::atomic::{AtomicU64, Ordering};

use bindings::exports::wasi::cli::run::Guest as RunGuest;
use bindings::wasi::cli::exit;

static RUNS: AtomicU64 = AtomicU64::new(0);

struct Component;

impl RunGuest for Component {
    async fn run() -> Result<(), ()> {
        let runs = RUNS.fetch_add(1, Ordering::SeqCst) + 1;
        let mode = std::env::var("RUN_MODE").unwrap_or_else(|_| "ok".to_string());
        eprintln!("cli-run-callee ran (runs={runs}, mode={mode})");
        match mode.as_str() {
            "err" => Err(()),
            // `exit` unwinds the instance, so neither of these returns.
            "exit-ok" => {
                exit::exit(Ok(()));
                Ok(())
            }
            "exit-err" => {
                exit::exit(Err(()));
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

bindings::export!(Component with_types_in bindings);
