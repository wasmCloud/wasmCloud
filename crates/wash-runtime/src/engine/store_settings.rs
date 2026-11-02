//! What every guest store an engine builds is set up with, whichever path
//! builds it: workload and service stores in
//! [`crate::engine::linked_call::new_store_from_templates`], plugin stores in
//! the component host. A new store-level wasmtime knob is a field on
//! [`StoreSettings`] and a line in [`configure_store`].

use std::num::NonZeroUsize;

use crate::engine::abandon::{AbandonedCallPolicy, arm_epoch_deadline};
use crate::engine::ctx::SharedCtx;

/// Engine-wide settings applied to each store as it is built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StoreSettings {
    /// wasmtime's [`wasmtime::Store::set_hostcall_fuel`]. `None` keeps
    /// wasmtime's default of 128 MiB.
    pub(crate) hostcall_fuel: Option<NonZeroUsize>,
}

/// Readies a freshly built store to run guests. `policy` is the one thing
/// that differs between callers: what an abandoned call ends in.
pub(crate) fn configure_store(
    store: &mut wasmtime::Store<SharedCtx>,
    settings: StoreSettings,
    policy: AbandonedCallPolicy,
) {
    // A store on a fuel-metering engine starts at zero, and calling a guest
    // without fuel traps. Fuel here is a counter, never a bound:
    // `FuelConsumptionMeter::observe` resets it and reads the delta around the
    // call it measures. Errors when the engine is not metering fuel, which is
    // the ordinary case.
    let _ = store.set_fuel(u64::MAX);
    // Required, not optional: the engine enables `epoch_interruption`, and a
    // store that never sets a deadline traps the moment it runs guest code.
    arm_epoch_deadline(store, policy);
    crate::engine::guest_memory::install_memory_limiter(store);
    if let Some(fuel) = settings.hostcall_fuel {
        store.set_hostcall_fuel(fuel.get());
    }
    store.data_mut().store_settings = settings;
}
