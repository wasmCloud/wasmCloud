//! Workload item a host plugin dispatches `acme:tasks/runner` calls to.
//!
//! Every reply counts the calls THIS instance has served, which is what makes
//! the fixture load-bearing: the count is instance state, so it says whether
//! the host reused an instance or built one for the call.
//!
//! - a component that keeps no instances warm answers `msg:1` every time (a
//!   fresh instance per call),
//! - a component with a warm pool answers `msg:1`, `msg:2`, ... (one instance
//!   serving them all), and
//! - a service answers the same way, on the one instance it already is.
//!
//! `wasi:cli/run` parks rather than returning, so deployed as a service this
//! stays the workload's long-lived item for as long as a test needs it.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "target", generate_all });
}

use std::sync::atomic::{AtomicU32, Ordering};

use bindings::exports::acme::tasks::runner::Guest as RunnerGuest;
use bindings::exports::wasi::cli::run::Guest as RunGuest;
use bindings::wasi::clocks::monotonic_clock;

/// Calls served by this instance. Lives in the instance's own linear memory, so
/// it restarts at zero in a fresh one.
static SERVED: AtomicU32 = AtomicU32::new(0);

struct Component;

impl RunnerGuest for Component {
    async fn run(message: String) -> String {
        // A dispatched call that does not complete, so a test can ask what
        // becomes of the instance that was serving it.
        assert!(message != "trap", "deliberate runner.run trap");
        let served = SERVED.fetch_add(1, Ordering::SeqCst) + 1;
        format!("{message}:{served}")
    }
}

impl RunGuest for Component {
    async fn run() -> Result<(), ()> {
        // `RUN_EXIT=error` fails the service's own long-running work while it is
        // still serving dispatched calls — the case where a workload's
        // `maxRestarts` has to mean the same thing whether or not a plugin
        // pushes into the service.
        match std::env::var("RUN_EXIT").as_deref() {
            Ok("error") => return Err(()),
            // The same failure the guest reports by trapping rather than by
            // answering, which a plain p3 service spends a restart on too.
            Ok("trap") => panic!("deliberate cli/run trap"),
            _ => {}
        }
        // Otherwise services are long-lived: park instead of returning, so the
        // workload stays deployed for as long as the test needs it.
        loop {
            monotonic_clock::wait_for(60_000_000_000).await;
        }
    }
}

mod export {
    #![allow(unsafe_code)]
    use super::{Component, bindings};
    bindings::export!(Component with_types_in bindings);
}
