//! Host component plugin with a deliberately-malformed
//! `wasmcloud:host/workload-lifecycle` export: its `on-workload-bind` takes a
//! bare string instead of the `workload-info` record and returns `u32` instead
//! of `result<_, string>`. Both mismatches are declared in this fixture's own
//! copy of the interface (`host-bad/workload-lifecycle.wit`) rather than the
//! canonical one, so the Rust signatures below are the ones wit-bindgen
//! generates from that wrong WIT — nothing is being coerced into the real
//! shape. Used only to assert that `ComponentHostPlugin::new` rejects the
//! mismatched signature at registration; the hooks are never invoked.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "badlifecycle", generate_all });
}

use bindings::exports::test::badlc::ops::Guest as OpsGuest;
use bindings::exports::wasmcloud::host::workload_lifecycle::Guest as LifecycleGuest;

struct Component;

impl OpsGuest for Component {
    fn ping() -> u32 {
        1
    }
}

impl LifecycleGuest for Component {
    fn on_workload_bind(_id: String) -> u32 {
        0
    }

    fn on_workload_unbind(_id: String) {}
}

mod export {
    #![allow(unsafe_code)]
    use super::{bindings, Component};
    bindings::export!(Component with_types_in bindings);
}
