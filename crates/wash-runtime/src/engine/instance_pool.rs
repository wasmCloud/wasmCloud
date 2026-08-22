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
//! of that: up to `pool_size` instances are kept, each owned by a long-lived
//! driver ([`crate::engine::instance_driver`]) that serves calls — inbound
//! HTTP and calls from other components alike — as concurrent tasks on its
//! instance, up to `max_concurrency` at a time. Guest state then survives
//! across calls, which is the entire point — but it is also why pooling is
//! opt-in rather than the default.
//!
//! What bounds an instance's life:
//!
//!  * `max_invocations` retires it after it has admitted that many calls
//!    (zero means no limit): it stops admitting, drains what it took, and is
//!    reaped, so guest state cannot accumulate forever. Until the drain
//!    finishes the instance still occupies its place in the pool, so a burst
//!    arriving mid-drain is served from one-shot stores. The drained instance
//!    ends its own run loop and drops its store there and then; the pool
//!    reaps the spent handle the next time it is offered a call.
//!  * A guest trap poisons the store, faulting the driver and every call in
//!    flight on that instance; the pool reaps it and the next call starts a
//!    fresh one. A call that times out or fails in the host mid-call retires
//!    the instance the same way — the guest work cannot be cancelled from the
//!    host, so draining and dropping the store is what ends it.
//!
//! Two further consequences of an instance outliving a call, both of which a
//! component opts into along with the pooling:
//!
//!  * The context it was built with (environment, config, resolved volume
//!    mounts) is frozen for its lifetime. `max_invocations` bounds how stale
//!    that can get.
//!  * A task the guest spawned but did not await lives on the store's
//!    concurrent state. Dropping the store per call used to discard it; a
//!    warm instance carries it forward, where it resumes alongside later
//!    calls. A guest that spawns background work and relies on it being torn
//!    down with the call should not be pooled.

use std::collections::{BTreeMap, HashSet};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use wasmtime::component::Instance;

use crate::engine::ctx::SharedCtx;
use crate::engine::instance_driver::{InstanceDriver, InstanceJob};
use crate::engine::workload::WorkloadComponent;
use crate::types::Component;

/// The pool that may keep `component_id`'s instances warm, or `None` when
/// every call must get a store of its own.
///
/// A store holds more than the one component: whatever that component is
/// linked to is instantiated alongside it and lives exactly as long as the
/// store does. Keeping the store warm therefore keeps *every* instance in it
/// warm, so every one of those components has to have opted in. Otherwise a
/// component that left `pool_size` at zero — saying its state is ephemeral —
/// would quietly acquire state that outlives a call, just because something
/// else in the workload imports it.
pub(crate) fn poolable(
    components: &BTreeMap<Arc<str>, WorkloadComponent>,
    component_id: &str,
    linked: &HashSet<Arc<str>>,
) -> Option<Arc<InstancePool>> {
    let pool = Arc::clone(&components.get(component_id)?.instances);
    if !pool.warms_instances() {
        return None;
    }
    for linked_id in linked {
        let linked_warms = components
            .get(linked_id)
            .is_some_and(|c| c.instances.warms_instances());
        if !linked_warms {
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

/// What [`InstancePool::offer`] did with a call.
pub(crate) enum Dispatch {
    /// A warm instance took it.
    Sent,
    /// Every warm instance is busy but the pool is under `pool_size`: build a
    /// store and hand both to [`InstancePool::install`].
    NeedsInstance(InstanceJob),
    /// Every warm instance is busy and the pool is full. Serve it from a store
    /// of its own — which is what an unpooled component pays for every call,
    /// so pooling never adds latency it did not save.
    Saturated(InstanceJob),
}

/// An instantiated component and the store it lives in.
///
/// Built by the caller, where instantiating is allowed to await and a failure
/// to instantiate can still be returned to whoever asked for the call. It then
/// either serves that one call and is dropped with it, or is handed to
/// [`InstancePool::install`] to be kept warm.
pub(crate) struct ComponentInstance {
    pub(crate) store: wasmtime::Store<SharedCtx>,
    pub(crate) instance: Instance,
}

/// What a component asked for by way of instance reuse.
///
/// The limits arrive as `sint32`, signed all the way from the Kubernetes CRD
/// a workload sets them through, and carry two sentinels between them —
/// negative for "the sender did not configure this" and zero for "no limit" —
/// so they are decoded into this once, at the edge, rather than being
/// re-interpreted at each use. Past this point the limits are `NonZeroUsize`
/// and the unset cases have names, so nothing downstream sees a signed
/// integer or has to decide what a negative one means.
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

    /// Guest calls this component may have in flight at once: every warm
    /// instance running its full `max_concurrency`. One for a component that
    /// keeps no instances — a burst past that is served by stores of its own,
    /// which this does not try to predict.
    ///
    /// Read by the outbound HTTP pool, whose connection burst scales with it
    /// (see `crate::host::http_client`).
    pub fn call_concurrency(&self) -> usize {
        match self {
            Self::Ephemeral => 1,
            Self::Warm {
                pool_size,
                max_concurrency,
                ..
            } => pool_size.get().saturating_mul(max_concurrency.get()),
        }
    }
}

/// The warm instances of one component, shared by every clone of its
/// [`crate::engine::workload::WorkloadComponent`] and therefore by every
/// importer that calls into it.
pub(crate) struct InstancePool {
    /// The component's warm instances. Each keeps its own store and serves
    /// calls concurrently (see [`crate::engine::instance_driver`]), whether
    /// they arrive over HTTP or from another component in the workload.
    drivers: Mutex<Vec<Arc<InstanceDriver>>>,
    policy: InstancePolicy,
}

impl InstancePool {
    pub(crate) fn new(policy: InstancePolicy) -> Self {
        Self {
            drivers: Mutex::new(Vec::new()),
            policy,
        }
    }

    /// Guest calls this component may have in flight at once. See
    /// [`InstancePolicy::call_concurrency`].
    pub(crate) fn call_concurrency(&self) -> usize {
        self.policy.call_concurrency()
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

    /// Offer a call to the warm instances.
    ///
    /// Picks the least-busy live one. Building a store is async and this runs
    /// under the pool's lock, so when the pool has room it hands the job back
    /// as [`Dispatch::NeedsInstance`] rather than creating one itself — that
    /// keeps a request that a warm instance can already serve from paying for
    /// a store it will not use.
    pub(crate) fn offer(&self, job: InstanceJob) -> Dispatch {
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
        // a trap takes down as little as possible. Sorted in place, since the
        // pool's order carries no meaning and leaving it sorted is what keeps
        // the next offer's sort near-linear.
        drivers.sort_by_key(|d| d.in_flight());

        let mut job = job;
        for driver in drivers.iter() {
            match driver.try_send(job) {
                Ok(()) => return Dispatch::Sent,
                Err(returned) => job = returned,
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
    pub(crate) fn install(
        &self,
        instance: ComponentInstance,
        job: InstanceJob,
    ) -> Result<(), InstanceJob> {
        let Some((pool_size, max_invocations, max_concurrency)) = self.limits() else {
            return Err(job);
        };
        let Ok(mut drivers) = self.drivers.lock() else {
            return Err(job);
        };
        if drivers.len() >= pool_size {
            drop(drivers);
            return match self.offer(job) {
                Dispatch::Sent => Ok(()),
                Dispatch::NeedsInstance(job) | Dispatch::Saturated(job) => Err(job),
            };
        }
        let driver = Arc::new(InstanceDriver::spawn(
            instance,
            max_concurrency,
            max_invocations,
        ));
        drivers.push(Arc::clone(&driver));
        drop(drivers);
        driver.try_send(job)
    }

    /// Whether this component keeps instances warm at all.
    pub(crate) fn warms_instances(&self) -> bool {
        self.policy.keeps_instances_warm()
    }

    /// Drop every warm instance, e.g. when the component is being shut down.
    /// Dropping a driver's handle closes its channel, which ends its store's
    /// run loop. This does not wait for a drain: calls still in flight on
    /// those instances end with the store they were running on.
    pub(crate) fn clear(&self) {
        if let Ok(mut drivers) = self.drivers.lock() {
            drop(std::mem::take(&mut *drivers));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InstancePolicy, NonZeroUsize};

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
}
