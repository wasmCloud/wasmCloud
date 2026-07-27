//! Instance pooling.
//!
//! By default a call runs in a store that is built, instantiated, invoked and
//! dropped per call, on the linked-call path (see
//! [`crate::engine::linked_call`]) and on HTTP dispatch alike. That keeps
//! component state ephemeral, which is the contract a `Component` is defined
//! by, but it also means the guest rebuilds everything it caches in linear
//! memory on every call: connection pools, lazily-built runtimes, parsed
//! configuration.
//!
//! A component whose [`InstancePolicy`] is [`InstancePolicy::Warm`] opts out
//! of that: after a call returns cleanly its store is parked here and the next
//! call to the same component reuses it, skipping the context build, the
//! [`wasmtime::Store`] allocation and the instantiation. Guest state then
//! survives across calls, which is the entire point — but it is also why
//! pooling is opt-in rather than the default.
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

use std::collections::{BTreeMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use wasmtime::component::Instance;

use crate::engine::ctx::SharedCtx;
use crate::engine::instance_driver::InstanceDriver;
use crate::engine::workload::WorkloadComponent;
use crate::host::http::ServiceHttpJob;
use crate::types::Component;

/// The pool that `component_id`'s store may be parked in, or `None` when it
/// must not be parked at all.
///
/// A store holds more than the one component: whatever that component is
/// linked to is instantiated alongside it and lives exactly as long as the
/// store does. Parking the store therefore keeps *every* instance in it warm,
/// so every one of those components has to have opted in. Otherwise a
/// component that left `pool_size` at zero — saying its state is ephemeral —
/// would quietly acquire state that outlives a call, just because something
/// else in the workload imports it.
pub(crate) fn poolable(
    components: &BTreeMap<Arc<str>, WorkloadComponent>,
    component_id: &str,
    linked: &HashSet<Arc<str>>,
) -> Option<Arc<InstancePool>> {
    let pool = Arc::clone(&components.get(component_id)?.instances);
    if !pool.enabled() {
        return None;
    }
    for linked_id in linked {
        let linked_enabled = components
            .get(linked_id)
            .is_some_and(|c| c.instances.enabled());
        if !linked_enabled {
            tracing::debug!(
                component_id,
                linked_id = linked_id.as_ref(),
                "not keeping instances warm: a linked component has not opted into pooling, \
                 and it shares the store's lifetime"
            );
            return None;
        }
    }
    Some(pool)
}

/// What [`InstancePool::offer_http`] did with a call.
pub(crate) enum Dispatch {
    /// A warm instance took it.
    Sent,
    /// Every warm instance is busy but the pool is under `pool_size`: build a
    /// store and hand both to [`InstancePool::install_http`].
    NeedsInstance(ServiceHttpJob),
    /// Every warm instance is busy and the pool is full. Serve it from a store
    /// of its own — which is what an unpooled component pays for every call,
    /// so pooling never adds latency it did not save.
    Saturated(ServiceHttpJob),
}

/// An instantiated component in its own store: what a call runs on, and what
/// the pool parks between calls when the component keeps instances warm.
pub(crate) struct ComponentInstance {
    pub(crate) store: wasmtime::Store<SharedCtx>,
    pub(crate) instance: Instance,
    /// Calls this instance has already served, counted against
    /// [`InstancePolicy::Warm::max_invocations`].
    pub(crate) invocations: usize,
}

/// What a component asked for by way of instance reuse.
///
/// `Component.pool_size` and `Component.max_invocations` arrive as `sint32`
/// and carry two sentinels between them — negative for "the sender did not
/// configure this" and zero for "no limit" — so they are decoded into this
/// once, at the edge, rather than being re-interpreted at each use.
/// Non-exhaustive in both directions on purpose: the limits a component can
/// state are expected to grow (how many calls one warm instance may serve at
/// once is next), and neither adding a variant nor adding a field to one
/// should be a breaking change. Build one with [`InstancePolicy::from_component`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InstancePolicy {
    /// No instance outlives the call it served. The default, and what keeps a
    /// component's state ephemeral.
    Ephemeral,
    /// Park up to `pool_size` instances between calls, retiring each once it
    /// has served `max_invocations` of them, and let each serve
    /// `max_concurrency` calls at a time.
    #[non_exhaustive]
    Warm {
        pool_size: NonZeroUsize,
        /// `None` when an instance may serve calls indefinitely.
        max_invocations: Option<NonZeroUsize>,
        /// Calls one instance may have in flight at once. One unless the
        /// component asked for more, which keeps the default identical to an
        /// unpooled component: a guest sees one call at a time unless it says
        /// it can take more.
        max_concurrency: NonZeroUsize,
    },
}

impl InstancePolicy {
    /// Read the policy a component declared.
    ///
    /// Takes the whole component rather than its limits so that a limit added
    /// to [`Component`] later reaches this without changing the signature.
    pub fn from_component(component: &Component) -> Self {
        Self::from_limits(
            component.pool_size,
            component.max_invocations,
            component.max_concurrency,
        )
    }

    /// Decode the wire limits. Anything that does not name a positive pool
    /// size, whether unset (`-1`), zero or negative, means instances are not
    /// kept; an unset `max_concurrency` means one call at a time.
    pub(crate) fn from_limits(pool_size: i32, max_invocations: i32, max_concurrency: i32) -> Self {
        let positive = |v: i32| usize::try_from(v).ok().and_then(NonZeroUsize::new);
        match positive(pool_size) {
            Some(pool_size) => Self::Warm {
                pool_size,
                max_invocations: positive(max_invocations),
                max_concurrency: positive(max_concurrency).unwrap_or(NonZeroUsize::MIN),
            },
            None => Self::Ephemeral,
        }
    }

    /// Whether this component keeps instances warm at all.
    pub fn keeps_instances_warm(&self) -> bool {
        matches!(self, Self::Warm { .. })
    }
}

/// The warm instances of one component, shared by every clone of its
/// [`crate::engine::workload::WorkloadComponent`] and therefore by every
/// importer that calls into it.
pub(crate) struct InstancePool {
    /// Warm instances parked between calls, each serving one call at a time.
    /// Used by the linked-call path.
    idle: Mutex<Vec<ComponentInstance>>,
    /// Warm instances that keep their own store and serve calls concurrently
    /// (see [`crate::engine::instance_driver`]). Used by HTTP dispatch, which
    /// is where a guest awaiting I/O would otherwise hold an instance idle.
    drivers: Mutex<Vec<Arc<InstanceDriver>>>,
    policy: InstancePolicy,
}

impl InstancePool {
    pub(crate) fn new(policy: InstancePolicy) -> Self {
        Self {
            idle: Mutex::new(Vec::new()),
            drivers: Mutex::new(Vec::new()),
            policy,
        }
    }

    /// The instance limits, or `None` when this component keeps none.
    fn limits(&self) -> Option<(usize, Option<usize>, usize)> {
        match self.policy {
            InstancePolicy::Ephemeral => None,
            InstancePolicy::Warm {
                pool_size,
                max_invocations,
                max_concurrency,
            } => Some((
                pool_size.get(),
                max_invocations.map(NonZeroUsize::get),
                max_concurrency.get(),
            )),
        }
    }

    /// Offer an HTTP call to the warm instances.
    ///
    /// Picks the least-busy live one. Building a store is async and this runs
    /// under the pool's lock, so when the pool has room it hands the job back
    /// as [`Dispatch::NeedsInstance`] rather than creating one itself — that
    /// keeps a request that a warm instance can already serve from paying for
    /// a store it will not use.
    pub(crate) fn offer_http(&self, job: ServiceHttpJob) -> Dispatch {
        let Some((pool_size, _, _)) = self.limits() else {
            return Dispatch::Saturated(job);
        };
        let Ok(mut drivers) = self.drivers.lock() else {
            return Dispatch::Saturated(job);
        };

        // Reap instances that have drained or whose store faulted, so a
        // retired one frees its place in the pool.
        drivers.retain(|d| !(d.is_gone() || d.is_retired() && d.in_flight() == 0));

        // Least-busy first: spread calls rather than filling one instance, so
        // a trap takes down as little as possible.
        let mut candidates: Vec<Arc<InstanceDriver>> = drivers.iter().map(Arc::clone).collect();
        candidates.sort_by_key(|d| d.in_flight());

        let mut job = job;
        for driver in &candidates {
            match driver.try_send(job) {
                Ok(()) => return Dispatch::Sent,
                Err(returned) => job = *returned,
            }
        }

        if drivers.len() < pool_size {
            Dispatch::NeedsInstance(job)
        } else {
            Dispatch::Saturated(job)
        }
    }

    /// Add an instance built for a [`Dispatch::NeedsInstance`] and give it the
    /// call. Racing callers can both be told to build one; the loser's is
    /// dropped and its call offered to the winner's instead.
    pub(crate) fn install_http(
        &self,
        store: wasmtime::Store<SharedCtx>,
        pre: wasmtime::component::InstancePre<SharedCtx>,
        job: ServiceHttpJob,
    ) -> Result<(), Box<ServiceHttpJob>> {
        let Some((pool_size, max_invocations, max_concurrency)) = self.limits() else {
            return Err(Box::new(job));
        };
        let Ok(mut drivers) = self.drivers.lock() else {
            return Err(Box::new(job));
        };
        if drivers.len() >= pool_size {
            drop(drivers);
            return match self.offer_http(job) {
                Dispatch::Sent => Ok(()),
                Dispatch::NeedsInstance(job) | Dispatch::Saturated(job) => Err(Box::new(job)),
            };
        }
        let driver = Arc::new(InstanceDriver::spawn(
            store,
            pre,
            max_concurrency,
            max_invocations,
        ));
        drivers.push(Arc::clone(&driver));
        drop(drivers);
        driver.try_send(job)
    }

    /// Whether this component keeps instances warm at all.
    pub(crate) fn enabled(&self) -> bool {
        self.policy.keeps_instances_warm()
    }

    /// Take a warm instance if one is parked. Returns `None` when pooling is
    /// disabled or every warm instance is already in use — the caller then
    /// builds a fresh store, so a burst past `pool_size` is served rather
    /// than queued.
    pub(crate) fn checkout(&self) -> Option<ComponentInstance> {
        if !self.enabled() {
            return None;
        }
        self.idle.lock().ok()?.pop()
    }

    /// Park an instance that has just served a call cleanly, unless it has
    /// reached `max_invocations` or the warm set is already full. Anything
    /// not parked is dropped here.
    pub(crate) fn release(&self, mut instance: ComponentInstance) {
        instance.invocations += 1;
        let InstancePolicy::Warm {
            pool_size,
            max_invocations,
            ..
        } = self.policy
        else {
            return;
        };
        if max_invocations.is_some_and(|limit| instance.invocations >= limit.get()) {
            trace_retire("reached max_invocations", instance.invocations);
            return;
        }
        // Drop the guard before the instance it rejected: dropping a store
        // tears down the guest's resources and should not hold the pool shut
        // while it runs.
        let rejected = {
            let Ok(mut idle) = self.idle.lock() else {
                return;
            };
            if idle.len() < pool_size.get() {
                idle.push(instance);
                None
            } else {
                Some(instance)
            }
        };
        if let Some(instance) = rejected {
            trace_retire("warm set full", instance.invocations);
        }
    }

    /// Drop every warm instance, e.g. when the component is being shut down.
    /// Dropping a driver's handle closes its channel, which ends its store's
    /// run loop once the calls it already took have finished.
    pub(crate) fn clear(&self) {
        if let Ok(mut idle) = self.idle.lock() {
            drop(std::mem::take(&mut *idle));
        }
        if let Ok(mut drivers) = self.drivers.lock() {
            drop(std::mem::take(&mut *drivers));
        }
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
    use super::{InstancePolicy, InstancePool, NonZeroUsize};

    /// Zero, and the `-1` that `wash dev` and the operator send for "not
    /// configured", both mean instances are not kept.
    #[test]
    fn absent_or_zero_pool_size_is_ephemeral() {
        for pool_size in [0, -1, i32::MIN] {
            assert_eq!(
                InstancePolicy::from_limits(pool_size, 0, 0),
                InstancePolicy::Ephemeral,
                "pool_size {pool_size} should not keep instances"
            );
        }
    }

    /// A positive pool size keeps instances; zero or unset `max_invocations`
    /// means an instance may serve calls indefinitely.
    #[test]
    fn positive_pool_size_is_warm() {
        assert_eq!(
            InstancePolicy::from_limits(4, 0, 0),
            InstancePolicy::Warm {
                pool_size: NonZeroUsize::new(4).unwrap(),
                max_invocations: None,
                max_concurrency: NonZeroUsize::MIN,
            }
        );
        assert_eq!(
            InstancePolicy::from_limits(4, -1, 0),
            InstancePolicy::Warm {
                pool_size: NonZeroUsize::new(4).unwrap(),
                max_invocations: None,
                max_concurrency: NonZeroUsize::MIN,
            }
        );
        assert_eq!(
            InstancePolicy::from_limits(2, 50, 0),
            InstancePolicy::Warm {
                pool_size: NonZeroUsize::new(2).unwrap(),
                max_invocations: NonZeroUsize::new(50),
                max_concurrency: NonZeroUsize::MIN,
            }
        );
    }

    #[test]
    fn disabled_pool_never_checks_out() {
        let pool = InstancePool::new(InstancePolicy::Ephemeral);
        assert!(!pool.enabled());
        assert!(pool.checkout().is_none());
    }

    #[test]
    fn enabled_pool_starts_empty() {
        let pool = InstancePool::new(InstancePolicy::from_limits(4, 0, 0));
        assert!(pool.enabled());
        assert_eq!(pool.idle_len(), 0);
        // Nothing parked yet, so a checkout still falls back to a cold store.
        assert!(pool.checkout().is_none());
    }
}
