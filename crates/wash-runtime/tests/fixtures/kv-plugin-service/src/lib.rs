//! Workload SERVICE driving the imported `acme:kv/store` capability from
//! `wasi:cli/run`.
//!
//! A service reaches a host component plugin through the same bind path a
//! component does, but it is the workload's long-lived item, so the host
//! reports it in `workload-info.service` rather than in `components`. On start
//! it writes a key the test reads back through a co-tenant caller workload —
//! proving both that the service bound and that a service can make cross-store
//! capability calls at all.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "kv-service", generate_all });
}

use bindings::acme::kv::store;
use bindings::exports::wasi::cli::run::Guest;
use bindings::wasi::clocks::monotonic_clock;

/// Key the service writes on start, holding the ambient identity the plugin
/// sees for the service's own capability call (`{workload-id}|{component-id}`).
/// The test reads it back through a co-tenant and checks the component half
/// against the id delivered as `workload-info.service` — so bind-time and
/// call-time agree on what the service is called. Written with the GLOBAL
/// `set` (not the per-caller `pset`) precisely so another workload can read it.
const WHOAMI_KEY: &str = "service-whoami";

struct Component;

impl Guest for Component {
    async fn run() -> Result<(), ()> {
        let whoami = store::whoami().await;
        store::set(WHOAMI_KEY.to_string(), whoami.into_bytes()).await;
        // Services are long-lived. Park instead of returning so the workload
        // stays deployed — and so its bind stays in the plugin's bound set —
        // for as long as the test needs it.
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
