//! Warm-instance pooling for ephemeral linked calls.
//!
//! By default a plain-value linked call runs in a store that is built,
//! instantiated, invoked and dropped per call (see
//! [`crate::engine::linked_call`]). That keeps component state ephemeral —
//! the contract a `Component` is defined by — but it also means the guest
//! rebuilds everything it caches in linear memory on every call: connection
//! pools, lazily-built runtimes, parsed configuration.
//!
//! A component that sets `pool_size > 0` opts out of that: after a call
//! returns cleanly its store is parked here and the next call to the same
//! component reuses it, skipping the context build, the [`wasmtime::Store`]
//! allocation and the instantiation. Guest state then survives across calls,
//! which is the entire point — but it is also why pooling is opt-in rather
//! than the default.
//!
//! Two rules bound how long any single instance lives:
//!
//!  * `max_invocations` retires an instance after it has served that many
//!    calls (zero means no limit), so guest state cannot accumulate forever
//!    and a workload still exercises the cold path it will hit in production.
//!  * An instance is parked **only** after a call that returned cleanly. A
//!    trap, a timeout, a host error, or a caller that was cancelled mid-call
//!    all retire the store instead, because a guest interrupted partway
//!    through a call may hold locks, half-written buffers or a protocol peer
//!    in an indeterminate state.
//!
//! Two further consequences of an instance outliving a call, both of which a
//! component opts into along with the pooling:
//!
//!  * The context it was built with (environment, config, resolved volume
//!    mounts) is frozen for its lifetime. `max_invocations` bounds how stale
//!    that can get.
//!  * A task the guest spawned but did not await lives on the store's
//!    concurrent state. Dropping the store per call used to discard it; a
//!    parked store carries it to the next call, where it resumes alongside
//!    that call's work. A guest that spawns background work and relies on it
//!    being torn down with the call should not be pooled.

use std::sync::Mutex;

use wasmtime::component::Instance;

use crate::engine::ctx::SharedCtx;

/// A store that has been instantiated and is parked between calls.
pub(crate) struct WarmInstance {
    pub(crate) store: wasmtime::Store<SharedCtx>,
    pub(crate) instance: Instance,
    /// Calls this instance has already served, counted against
    /// [`InstancePool::max_invocations`].
    pub(crate) invocations: usize,
}

/// The warm instances of one component, shared by every clone of its
/// [`crate::engine::workload::WorkloadComponent`] and therefore by every
/// importer that calls into it.
pub(crate) struct InstancePool {
    idle: Mutex<Vec<WarmInstance>>,
    /// How many warm instances to park. Zero disables pooling: every call
    /// builds and drops its own store.
    pool_size: usize,
    /// Calls an instance serves before it is retired. Zero means unlimited.
    max_invocations: usize,
}

impl InstancePool {
    /// Build a pool from a component's configured limits. Both fields are
    /// `sint32` on the wire and are `-1` when unset, so anything below zero
    /// reads as "not configured".
    pub(crate) fn new(pool_size: i32, max_invocations: i32) -> Self {
        Self {
            idle: Mutex::new(Vec::new()),
            pool_size: pool_size.max(0) as usize,
            max_invocations: max_invocations.max(0) as usize,
        }
    }

    /// Whether this component keeps instances warm at all.
    pub(crate) fn enabled(&self) -> bool {
        self.pool_size > 0
    }

    /// Take a warm instance if one is parked. Returns `None` when pooling is
    /// disabled or every warm instance is already in use — the caller then
    /// builds a fresh store, so a burst past `pool_size` is served rather
    /// than queued.
    pub(crate) fn checkout(&self) -> Option<WarmInstance> {
        if !self.enabled() {
            return None;
        }
        self.idle.lock().ok()?.pop()
    }

    /// Park an instance that has just served a call cleanly, unless it has
    /// reached `max_invocations` or the warm set is already full. Anything
    /// not parked is dropped here.
    pub(crate) fn release(&self, mut warm: WarmInstance) {
        warm.invocations += 1;
        if !self.enabled() {
            return;
        }
        if self.max_invocations > 0 && warm.invocations >= self.max_invocations {
            trace_retire("reached max_invocations", warm.invocations);
            return;
        }
        // Drop the guard before the `WarmInstance` it rejected: dropping a
        // store tears down the guest's resources and should not hold the pool
        // shut while it runs.
        let rejected = {
            let Ok(mut idle) = self.idle.lock() else {
                return;
            };
            if idle.len() < self.pool_size {
                idle.push(warm);
                None
            } else {
                Some(warm)
            }
        };
        if let Some(warm) = rejected {
            trace_retire("warm set full", warm.invocations);
        }
    }

    /// Drop every parked instance, e.g. when the component is being shut
    /// down. In-flight calls are unaffected; they own their stores.
    pub(crate) fn clear(&self) {
        let parked = match self.idle.lock() {
            Ok(mut idle) => std::mem::take(&mut *idle),
            Err(_) => return,
        };
        drop(parked);
    }

    /// How many instances are parked right now. Test/observability only.
    #[cfg(test)]
    pub(crate) fn idle_len(&self) -> usize {
        self.idle.lock().map(|idle| idle.len()).unwrap_or(0)
    }
}

fn trace_retire(reason: &str, invocations: usize) {
    tracing::trace!(reason, invocations, "retiring warm instance");
}

#[cfg(test)]
mod tests {
    use super::InstancePool;

    #[test]
    fn disabled_by_default() {
        let pool = InstancePool::new(0, 0);
        assert!(!pool.enabled());
        assert!(pool.checkout().is_none());
    }

    #[test]
    fn unset_limits_read_as_disabled() {
        // `wash dev` and the operator send -1 for "not configured".
        let pool = InstancePool::new(-1, -1);
        assert!(!pool.enabled());
    }

    #[test]
    fn enabled_pool_starts_empty() {
        let pool = InstancePool::new(4, 0);
        assert!(pool.enabled());
        assert_eq!(pool.idle_len(), 0);
        // Nothing parked yet, so a checkout still falls back to a cold store.
        assert!(pool.checkout().is_none());
    }
}
