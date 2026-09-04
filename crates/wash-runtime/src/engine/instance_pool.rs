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
//!    reaps the spent handle the next time a call is dispatched to it.
//!  * A guest trap poisons the store, faulting the driver and every call in
//!    flight on that instance; the pool reaps it and the next call starts a
//!    fresh one. A call that times out or fails in the host mid-call retires
//!    the instance the same way — the guest work cannot be cancelled from the
//!    host, so draining and dropping the store is what ends it.
//!  * A [`ReclaimPolicy`], when the component asked for one, retires what the
//!    pool's own recent peak did not need. Without it a pool grows to
//!    `pool_size` under load and stays there, so a spike's high-water mark
//!    outlives the spike and `max_invocations` is the only decay available.
//!    See [`InstancePool::sweep`] for why the measurement is of the pool
//!    rather than of each instance.
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
use std::sync::{Arc, Mutex, MutexGuard, Once};
use std::time::Duration;

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

/// What [`InstancePool::try_dispatch`] did with a call.
pub(crate) enum Dispatch {
    /// A warm instance took it.
    Sent,
    /// Every warm instance is busy but the pool is under `pool_size`: build a
    /// store and hand both to [`InstancePool::dispatch_on_new`].
    NeedsInstance(InstanceJob),
    /// Every warm instance is busy and the pool is full. Serve it from a store
    /// of its own — which is what an unpooled component pays for every call,
    /// so pooling never adds latency it did not save.
    Saturated(InstanceJob),
}

/// A call [`InstancePool::dispatch_on_new`] would not take, and the instance
/// built for it — so the caller's own store path runs on that rather than
/// instantiating a second time. `None` when there is none to give back: the
/// pool parked the instance and the call still did not fit on it, or the call
/// was declined before one was built.
pub(crate) struct Declined {
    pub(crate) job: InstanceJob,
    pub(crate) instance: Option<ComponentInstance>,
}

impl Declined {
    /// A declined call with no instance to give back.
    pub(crate) fn without_instance(job: InstanceJob) -> Self {
        Self {
            job,
            instance: None,
        }
    }

    /// A declined call and the instance the caller built for it.
    fn with_instance(job: InstanceJob, instance: ComponentInstance) -> Self {
        Self {
            job,
            instance: Some(instance),
        }
    }
}

/// An instantiated component and the store it lives in.
///
/// Built by the caller, where instantiating is allowed to await and a failure
/// to instantiate can still be returned to whoever asked for the call. It then
/// either serves that one call and is dropped with it, or is handed to
/// [`InstancePool::dispatch_on_new`] — to be kept warm, or to come back in a
/// [`Declined`] and serve the call the pool would not take.
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
        /// `None` when a warm instance is never reclaimed for idleness, which
        /// is what a component gets without asking: the pool then keeps
        /// whatever its busiest moment ever needed.
        reclaim: Option<ReclaimPolicy>,
    },
}

/// When a pool gives instances back.
///
/// The pool measures how many calls it was holding at once, and every
/// `window` retires the instances that peak did not need, never going below
/// `min_instances`. The sweep itself is `InstancePool::sweep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReclaimPolicy {
    /// How long each sweep watches the pool's peak concurrency.
    pub window: Duration,
    /// Warm instances a sweep never retires below. Zero lets an idle pool
    /// empty out, so the next call after a quiet spell starts cold. Always
    /// below `pool_size`: a floor that reaches it is decoded as no reclaim at
    /// all (see [`InstancePolicy::from_component`]).
    ///
    /// A floor on reclaim, not a target: nothing here builds an instance, so
    /// a pool that has never served a call stays empty whatever this says.
    pub min_instances: usize,
}

impl InstancePolicy {
    /// Read the policy a component declared, decoding the wire limits in the
    /// one place they are decoded.
    ///
    /// Anything that does not name a positive pool size, whether unset
    /// (`-1`), zero or negative, means instances are not kept; an unset
    /// `max_concurrency` means one call at a time; an unset reclaim window
    /// means a warm instance is never given back for idleness. Takes the
    /// whole component rather than its limits so that a limit added to
    /// [`Component`] later reaches this without changing the signature.
    pub fn from_component(component: &Component) -> Self {
        let positive = |v: i32| usize::try_from(v).ok().and_then(NonZeroUsize::new);
        let Some(pool_size) = positive(component.pool_size) else {
            return Self::Ephemeral;
        };
        Self::Warm {
            pool_size,
            max_invocations: positive(component.max_invocations),
            max_concurrency: positive(component.max_concurrency).unwrap_or(NonZeroUsize::MIN),
            reclaim: positive(component.reclaim_window_seconds).and_then(|window| {
                // A floor at or above the pool size names a pool that can
                // never shrink. Saying so here is what keeps a sweep that
                // could only ever find a surplus of zero from being started
                // at all.
                let min_instances = usize::try_from(component.reclaim_min_instances).unwrap_or(0);
                (min_instances < pool_size.get()).then_some(ReclaimPolicy {
                    window: Duration::from_secs(window.get() as u64),
                    min_instances,
                })
            }),
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
    state: Mutex<PoolState>,
    policy: InstancePolicy,
    /// Starts the sweep with the pool's first instance. There is nothing to
    /// reclaim before that, and [`Self::dispatch_on_new`] is the one entry
    /// point guaranteed to run inside the tokio runtime the sweep needs.
    sweep_started: Once,
}

/// What the pool's lock guards: the warm instances, and the measurement the
/// sweep sizes them by.
struct PoolState {
    /// The component's warm instances. Each keeps its own store and serves
    /// calls concurrently (see [`crate::engine::instance_driver`]), whether
    /// they arrive over HTTP or from another component in the workload.
    drivers: Vec<Arc<InstanceDriver>>,
    /// The most calls this pool has been asked to hold at once since the
    /// last sweep. Raised by [`InstancePool::try_dispatch`] and
    /// [`InstancePool::dispatch_on_new`], read and reset by
    /// [`InstancePool::sweep`].
    peak_in_flight: usize,
}

impl InstancePool {
    pub(crate) fn new(policy: InstancePolicy) -> Self {
        Self {
            state: Mutex::new(PoolState {
                drivers: Vec::new(),
                peak_in_flight: 0,
            }),
            policy,
            sweep_started: Once::new(),
        }
    }

    /// Lock the pool's state, recovering it if a panic poisoned the lock.
    ///
    /// Treating a poisoned lock as "no pool" would disable pooling for the
    /// rest of the workload's life: every later lock would fail, and every
    /// call would quietly fall through to the cold path — still serving 200s,
    /// at unpooled throughput, with nothing surfaced to the operator. An
    /// unwind cannot leave the state logically inconsistent — a panicking sort
    /// still leaves a valid permutation, and the other critical sections only
    /// push, retain or take the `Vec` and assign the peak — so the state is
    /// safe to keep using: clear the poison and keep serving warm instances.
    fn lock_state(&self) -> MutexGuard<'_, PoolState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                self.state.clear_poison();
                tracing::warn!(
                    "a panic poisoned the instance pool's lock; recovering the warm pool"
                );
                poisoned.into_inner()
            }
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
                ..
            } => Some((
                pool_size.get(),
                max_invocations.map(NonZeroUsize::get),
                max_concurrency.get(),
            )),
        }
    }

    /// When this pool gives instances back, or `None` when it never does.
    fn reclaim_policy(&self) -> Option<ReclaimPolicy> {
        match self.policy {
            InstancePolicy::Ephemeral => None,
            InstancePolicy::Warm { reclaim, .. } => reclaim,
        }
    }

    /// Offer a call to the warm instances.
    ///
    /// Picks the least-busy live one. Building a store is async and this runs
    /// under the pool's lock, so when the pool has room it hands the job back
    /// as [`Dispatch::NeedsInstance`] rather than creating one itself — that
    /// keeps a request that a warm instance can already serve from paying for
    /// a store it will not use.
    pub(crate) fn try_dispatch(&self, job: InstanceJob) -> Dispatch {
        let Some((pool_size, _, _)) = self.limits() else {
            return Dispatch::Saturated(job);
        };
        let reclaims = self.reclaim_policy().is_some();
        let mut state = self.lock_state();
        let PoolState {
            drivers,
            peak_in_flight,
        } = &mut *state;
        reap(drivers);

        // This call on top of what the warm instances already hold. A lower
        // bound: calls arriving together do not see each other here, so
        // `dispatch_on_new` samples again once its instance is in place. Only
        // a pool that reclaims pays to measure itself.
        if reclaims {
            *peak_in_flight = (*peak_in_flight).max(in_flight_total(drivers) + 1);
        }

        // Least-busy first: spread calls rather than filling one instance, so
        // a trap takes down as little as possible. Sorted in place, since the
        // pool's order carries no meaning and leaving it sorted is what keeps
        // the next dispatch's sort near-linear.
        sort_least_busy(drivers);

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
    /// call — unless a warm instance can take the call by now.
    ///
    /// Every caller in a burst at a cold pool is told to build one. Adding
    /// them all would keep instances the burst's concurrency never needed:
    /// the sweep would retire them a window later, and the next burst would
    /// build them all over again. So an instance whose call fits on one
    /// already warm is dropped instead, after the lock is released.
    ///
    /// A call the pool cannot take at all is the other case: there the
    /// instance comes back in the [`Declined`], and the caller runs the call
    /// on it rather than instantiating a second time for the same call.
    pub(crate) fn dispatch_on_new(
        self: &Arc<Self>,
        instance: ComponentInstance,
        job: InstanceJob,
    ) -> Result<(), Declined> {
        let Some((pool_size, max_invocations, max_concurrency)) = self.limits() else {
            return Err(Declined::with_instance(job, instance));
        };
        let reclaims = self.reclaim_policy().is_some();
        let mut state = self.lock_state();
        let PoolState {
            drivers,
            peak_in_flight,
        } = &mut *state;

        let sent = 'sent: {
            let mut job = job;
            for driver in drivers.iter() {
                match driver.try_send(job) {
                    Ok(()) => break 'sent Ok(()),
                    Err(returned) => job = returned,
                }
            }
            if drivers.len() >= pool_size {
                break 'sent Err(Declined::with_instance(job, instance));
            }
            let driver = Arc::new(InstanceDriver::spawn(
                instance,
                max_concurrency,
                max_invocations,
            ));
            drivers.push(Arc::clone(&driver));
            // Sent under the lock, as `try_dispatch` sends: a sweep landing
            // between the push and the send would find an idle instance
            // nothing had claimed yet and retire it out from under this call.
            // The instance is parked whether or not it takes the call, so a
            // refusal here has none to hand back.
            driver.try_send(job).map_err(Declined::without_instance)
        };

        // Sampled again with the call in place: at `try_dispatch` it may have
        // seen an empty pool its siblings were about to fill.
        if reclaims {
            *peak_in_flight = (*peak_in_flight).max(in_flight_total(drivers));
        }
        drop(state);
        self.start_sweeping();
        sent
    }

    /// Start the periodic sweep that gives idle instances back, once, with
    /// the pool's first instance.
    ///
    /// A sweep driven from [`Self::try_dispatch`] would never run for the
    /// pool that most needs it: a component whose traffic stopped altogether
    /// dispatches nothing, and its whole spike-sized pool would sit warm
    /// until the workload stopped. The timer is the pool's own rather than
    /// one the host drives over a registry of pools, so it lives and dies
    /// with the pool it sweeps and needs nothing plumbed through the engine —
    /// the cost is one sleeping task per pooled component that asked to be
    /// reclaimed.
    fn start_sweeping(self: &Arc<Self>) {
        let Some(reclaim) = self.reclaim_policy() else {
            return;
        };
        self.sweep_started.call_once(|| {
            // Weak, so a stopped workload's pool is not kept alive by its own
            // timer.
            let pool = Arc::downgrade(self);
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(reclaim.window);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                // `interval` yields its first tick immediately, before any
                // window has passed to measure.
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    // Nothing holds the pool any more: its workload stopped,
                    // and its instances went with it.
                    let Some(alive) = pool.upgrade() else { return };
                    alive.sweep();
                }
            });
        });
    }

    /// Retire the warm instances the last window's peak concurrency did not
    /// need, down to the policy's floor.
    ///
    /// The measurement is of the pool, not of each instance. A per-instance
    /// idle timer never fires here: [`Self::try_dispatch`] spreads calls
    /// least-busy first, so steady low traffic touches every instance in turn
    /// and none of them is ever idle for long. Eight instances serving one
    /// call a second each see a call every eight seconds and would all
    /// persist. What the pool can say is how many calls it held at once, and
    /// `max_concurrency` turns that into the number of instances that peak
    /// actually needed.
    ///
    /// Retiring drains: an instance stops admitting, finishes what it took
    /// and only then drops its store, so a sweep never ends a call in flight.
    fn sweep(&self) {
        let (Some((pool_size, _, max_concurrency)), Some(reclaim)) =
            (self.limits(), self.reclaim_policy())
        else {
            return;
        };
        let mut state = self.lock_state();
        let PoolState {
            drivers,
            peak_in_flight,
        } = &mut *state;
        reap(drivers);

        // The next window starts from the work in flight right now. A call
        // is sampled once, as it arrives, so a pool serving nothing but calls
        // that outlive a window would otherwise measure a peak of zero and
        // retire the instances running them.
        let in_flight = in_flight_total(drivers);
        let peak = std::mem::replace(peak_in_flight, in_flight);
        // The peak counts calls the warm set could not take, which overflow
        // to stores of their own, so cap what it asks for at the pool size
        // before the floor applies.
        let needed = peak
            .div_ceil(max_concurrency)
            .min(pool_size)
            .max(reclaim.min_instances);

        // Only idle instances go. `needed` assumes calls pack at full
        // density, but `try_dispatch` spreads them, so above one call per
        // instance the surplus can name an instance mid-call — and its warm
        // state would go with it. The next window takes what this one leaves.
        let live = drivers.iter().filter(|d| !d.is_retired()).count();
        let surplus = live.saturating_sub(needed);
        if surplus == 0 {
            return;
        }
        tracing::debug!(
            peak,
            needed,
            live,
            surplus,
            "retiring the warm instances the pool's recent peak did not need"
        );
        for driver in drivers
            .iter()
            .filter(|d| !d.is_retired() && d.in_flight() == 0)
            .take(surplus)
        {
            driver.retire();
        }
    }

    /// Whether this component keeps instances warm at all.
    pub(crate) fn warms_instances(&self) -> bool {
        self.policy.keeps_instances_warm()
    }

    /// The policy this pool was built with, for a dispatch path that keeps its
    /// own warm set (the `wasmcloud:nats` subscriber) but must honour the same
    /// declaration.
    #[cfg_attr(not(feature = "wasmcloud-nats"), allow(dead_code))]
    pub(crate) fn policy(&self) -> InstancePolicy {
        self.policy
    }

    /// Drop every warm instance, e.g. when the component is being shut down.
    /// Dropping a driver's handle closes its channel, which ends its store's
    /// run loop. This does not wait for a drain: calls still in flight on
    /// those instances end with the store they were running on.
    pub(crate) fn clear(&self) {
        let mut state = self.lock_state();
        drop(std::mem::take(&mut state.drivers));
    }
}

/// Calls in flight across `drivers`.
fn in_flight_total(drivers: &[Arc<InstanceDriver>]) -> usize {
    drivers.iter().map(|d| d.in_flight()).sum()
}

/// Drop the instances that have drained or whose store faulted, so a retired
/// one frees its place in the pool. Shared by the dispatch path and the sweep
/// so the two cannot disagree about what still counts as an instance.
fn reap(drivers: &mut Vec<Arc<InstanceDriver>>) {
    drivers.retain(|d| !(d.is_gone() || d.is_retired() && d.in_flight() == 0));
}

/// Order drivers least-busy first, comparing a snapshot of each driver's
/// in-flight count rather than the live counter.
///
/// Calls start and finish on other threads throughout the sort, so a
/// comparator that re-read [`InstanceDriver::in_flight`] per comparison can
/// observe the same driver under different keys within one pass. That violates
/// the strict weak ordering the sort requires, which Rust's sort detects by
/// panicking ("user-provided comparison function does not correctly implement
/// a total order") — and the unwind then poisons the pool's lock.
/// `sort_by_cached_key` computes every key exactly once, up front, so the
/// ordering it compares cannot shift mid-sort.
fn sort_least_busy(drivers: &mut [Arc<InstanceDriver>]) {
    drivers.sort_by_cached_key(|d| d.in_flight());
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{Duration, InstancePolicy, InstancePool, NonZeroUsize, sort_least_busy};
    use crate::engine::instance_driver::{InFlightGuard, InstanceDriver, InstanceJob};
    use crate::types::Component;

    /// The limits a component declared, as the wire carries them.
    fn limits(
        pool_size: i32,
        max_invocations: i32,
        max_concurrency: i32,
        reclaim_window_seconds: i32,
        reclaim_min_instances: i32,
    ) -> Component {
        Component {
            pool_size,
            max_invocations,
            max_concurrency,
            reclaim_window_seconds,
            reclaim_min_instances,
            ..Default::default()
        }
    }

    /// Zero, and the `-1` that `wash dev` and the operator send for "not
    /// configured", both mean instances are not kept.
    #[test]
    fn absent_or_zero_pool_size_is_ephemeral() {
        for pool_size in [0, -1, i32::MIN] {
            assert_eq!(
                InstancePolicy::from_component(&limits(pool_size, 0, 0, 0, 0)),
                InstancePolicy::Ephemeral,
                "pool_size {pool_size} should not keep instances"
            );
        }
    }

    /// A positive pool size keeps instances; zero or unset `max_invocations`
    /// means an instance may serve calls indefinitely, and an unset reclaim
    /// window means a warm instance is never given back for idleness.
    #[test]
    fn positive_pool_size_is_warm() {
        assert_eq!(
            InstancePolicy::from_component(&limits(4, 0, 0, 0, 0)),
            InstancePolicy::Warm {
                pool_size: NonZeroUsize::new(4).unwrap(),
                max_invocations: None,
                max_concurrency: NonZeroUsize::MIN,
                reclaim: None,
            }
        );
        assert_eq!(
            InstancePolicy::from_component(&limits(4, -1, 0, -1, -1)),
            InstancePolicy::Warm {
                pool_size: NonZeroUsize::new(4).unwrap(),
                max_invocations: None,
                max_concurrency: NonZeroUsize::MIN,
                reclaim: None,
            }
        );
        assert_eq!(
            InstancePolicy::from_component(&limits(2, 50, 0, 0, 0)),
            InstancePolicy::Warm {
                pool_size: NonZeroUsize::new(2).unwrap(),
                max_invocations: NonZeroUsize::new(50),
                max_concurrency: NonZeroUsize::MIN,
                reclaim: None,
            }
        );
    }

    /// The reclaim settings a component declared, or `None` where it asked
    /// for no reclaim.
    fn reclaim_policy_of(component: &Component) -> Option<super::ReclaimPolicy> {
        match InstancePolicy::from_component(component) {
            InstancePolicy::Warm { reclaim, .. } => reclaim,
            InstancePolicy::Ephemeral => panic!("a positive pool size keeps instances"),
        }
    }

    /// A reclaim window turns the sweep on; an unset floor lets an idle pool
    /// empty out; and a floor that reaches the pool size names a pool that
    /// can never shrink, which is decoded as no reclaim rather than as a
    /// sweep that could only ever find a surplus of zero.
    #[test]
    fn a_reclaim_window_and_its_floor_are_decoded() {
        assert_eq!(
            reclaim_policy_of(&limits(4, 0, 0, 30, 2)).map(|r| (r.window, r.min_instances)),
            Some((Duration::from_secs(30), 2))
        );
        assert_eq!(
            reclaim_policy_of(&limits(4, 0, 0, 30, -1)).map(|r| r.min_instances),
            Some(0),
            "an unset floor lets an idle pool empty out"
        );
        for floor in [4, 9] {
            assert_eq!(
                reclaim_policy_of(&limits(4, 0, 0, 30, floor)),
                None,
                "a floor of {floor} over a pool of 4 can never reclaim anything"
            );
        }
    }

    /// Ordering the pool least-busy first must tolerate in-flight counts
    /// moving under it: calls start and finish on other threads for the whole
    /// time `try_dispatch` sorts. When the comparator re-read the live counter
    /// comparison, that churn made the ordering inconsistent within a single
    /// pass, and Rust's sort raised "user-provided comparison function does
    /// not correctly implement a total order" — poisoning the pool's lock and
    /// silently disabling pooling. The keys are snapshotted now; this drives
    /// the old race, which panicked here before the fix.
    #[test]
    fn sorting_survives_concurrent_in_flight_churn() {
        let mut drivers = Vec::new();
        let mut channels = Vec::new();
        for _ in 0..32 {
            let (driver, rx) = InstanceDriver::stub(64, None);
            drivers.push(Arc::new(driver));
            // Keep the receivers so the drivers stay live for the whole test.
            channels.push(rx);
        }

        // Ends the churn threads however the sorting ends: a panicking sort
        // must fail the test, not leave the threads spinning on a flag the
        // unwound closure can no longer set.
        struct StopOnDrop<'a>(&'a AtomicBool);
        impl Drop for StopOnDrop<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }

        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let stop = &stop;
            for chunk in drivers.chunks(8) {
                scope.spawn(move || {
                    let mut guards = Vec::new();
                    while !stop.load(Ordering::Relaxed) {
                        for driver in chunk {
                            guards.extend(driver.try_admit());
                        }
                        guards.clear();
                    }
                });
            }

            let _stop = StopOnDrop(stop);
            let mut pool: Vec<_> = drivers.iter().map(Arc::clone).collect();
            for _ in 0..50_000 {
                sort_least_busy(&mut pool);
            }
        });
    }

    /// A pool holding `count` idle warm instances under the given limits.
    /// The stubs' receivers come back with it: dropping one closes its
    /// channel, which the pool reads as an instance that has gone.
    fn warm_pool(
        count: usize,
        limits: &Component,
    ) -> (
        Arc<InstancePool>,
        Vec<tokio::sync::mpsc::Receiver<(InstanceJob, InFlightGuard)>>,
    ) {
        let policy = InstancePolicy::from_component(limits);
        let InstancePolicy::Warm {
            max_concurrency, ..
        } = policy
        else {
            panic!("a warm pool needs a positive pool size");
        };
        let pool = Arc::new(InstancePool::new(policy));
        let max_concurrency = max_concurrency.get();
        let mut channels = Vec::with_capacity(count);
        for _ in 0..count {
            let (driver, rx) = InstanceDriver::stub(max_concurrency, None);
            pool.lock_state().drivers.push(Arc::new(driver));
            channels.push(rx);
        }
        (pool, channels)
    }

    /// Instances still admitting calls.
    fn live(pool: &InstancePool) -> usize {
        pool.lock_state()
            .drivers
            .iter()
            .filter(|d| !d.is_retired())
            .count()
    }

    /// A sweep keeps the instances the window's peak concurrency needed and
    /// retires the rest. The peak is of the pool rather than of each instance
    /// because `try_dispatch` spreads calls least-busy first: eight instances
    /// serving one call a second see a call every eight seconds, so no
    /// per-instance idle timer would ever fire.
    #[test]
    fn a_sweep_keeps_what_the_peak_needed() {
        let (pool, _channels) = warm_pool(8, &limits(8, 0, 1, 60, 0));

        pool.lock_state().peak_in_flight = 3;
        pool.sweep();
        assert_eq!(live(&pool), 3, "a peak of three needs three instances");

        // The window that follows saw nothing at all.
        pool.sweep();
        assert_eq!(live(&pool), 0, "a pool nothing asked for empties out");
    }

    /// `max_concurrency` is what turns a peak call count into an instance
    /// count: nine concurrent calls fit on three instances serving four each.
    #[test]
    fn a_sweep_sizes_the_peak_by_max_concurrency() {
        let (pool, _channels) = warm_pool(8, &limits(8, 0, 4, 60, 0));

        pool.lock_state().peak_in_flight = 9;
        pool.sweep();
        assert_eq!(live(&pool), 3);
    }

    /// The floor is what a component that would rather keep instances warm
    /// through the quiet asks for; without one an idle pool empties out.
    #[test]
    fn a_sweep_never_retires_below_the_floor() {
        let (pool, _channels) = warm_pool(8, &limits(8, 0, 1, 60, 2));

        pool.sweep();
        assert_eq!(
            live(&pool),
            2,
            "an idle pool falls to its floor, not to zero"
        );
        pool.sweep();
        assert_eq!(live(&pool), 2, "and stays there");
    }

    /// Calls in flight when the sweep runs are counted, however long ago they
    /// arrived. A call is sampled once, as it arrives, so a pool serving
    /// nothing but calls that outlive a window would otherwise measure a peak
    /// of zero and retire the instances running them.
    #[test]
    fn a_sweep_counts_the_calls_still_running() {
        let (pool, _channels) = warm_pool(4, &limits(4, 0, 1, 60, 0));

        // Two instances are mid-call, and nothing has arrived this window.
        let busy: Vec<_> = pool
            .lock_state()
            .drivers
            .iter()
            .take(2)
            .map(|d| d.try_admit().expect("an idle stub admits"))
            .collect();

        pool.sweep();
        assert_eq!(live(&pool), 2, "the instances serving calls must survive");
        assert!(
            pool.lock_state()
                .drivers
                .iter()
                .all(|d| d.is_retired() == (d.in_flight() == 0)),
            "the surplus must come off the idle instances, not the busy ones"
        );

        // Those calls end. The window they were still running in ends with
        // them holding two instances, so it is the window after that — the
        // first to hold nothing from beginning to end — which gives them back.
        drop(busy);
        pool.sweep();
        assert_eq!(live(&pool), 2, "the window those calls ran in needed two");
        pool.sweep();
        assert_eq!(
            live(&pool),
            0,
            "the first window holding nothing gives them back"
        );
    }

    /// `needed` is what the peak would take at full density, but
    /// `try_dispatch` spreads calls rather than packing them, so with
    /// `max_concurrency`
    /// above one the surplus can name instances that are serving right now.
    /// Retiring one of those would throw away the warm state it built for the
    /// call it is in the middle of, so the sweep leaves it and takes it next
    /// window instead.
    #[test]
    fn a_sweep_leaves_an_instance_that_is_serving() {
        let (pool, _channels) = warm_pool(4, &limits(4, 0, 4, 60, 0));

        // Four calls spread one per instance, as `try_dispatch` would place them.
        // At four calls a window, two instances could hold them all.
        let mut busy: Vec<_> = pool
            .lock_state()
            .drivers
            .iter()
            .map(|d| d.try_admit().expect("an idle stub admits"))
            .collect();

        pool.sweep();
        assert_eq!(
            live(&pool),
            4,
            "no instance was idle, so the sweep must retire none of them"
        );

        // One call ends, and its instance is the one the next sweep takes.
        drop(busy.pop().expect("four calls were admitted"));
        pool.sweep();
        assert_eq!(live(&pool), 3);
    }

    /// A component that asked for no reclaim keeps what it had: the sweep
    /// never runs, and `try_dispatch` does not even sample the pool for it.
    #[test]
    fn without_a_window_nothing_is_reclaimed() {
        let (pool, _channels) = warm_pool(4, &limits(4, 0, 1, 0, 0));

        pool.sweep();
        assert_eq!(live(&pool), 4);
        assert_eq!(
            pool.lock_state().peak_in_flight,
            0,
            "a pool that never reclaims must not pay to measure itself"
        );
    }

    /// A panic while the pool's lock is held must not disable pooling. It
    /// used to: the poisoned lock made every later dispatch fall
    /// through to the cold path — still serving 200s, at unpooled throughput,
    /// until the workload was restarted.
    #[test]
    fn a_panic_under_the_lock_does_not_disable_the_pool() {
        let pool = InstancePool::new(InstancePolicy::from_component(&limits(4, 0, 0, 0, 0)));

        // Panic while holding the lock, as the key-rereading sort in
        // `try_dispatch` did. The hook is silenced so the intended panic does
        // not land in the test output as a failure's would.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = pool.state.lock().expect("lock is not yet poisoned");
            panic!("simulated panic while holding the pool's lock");
        }));
        std::panic::set_hook(hook);
        assert!(
            pool.state.is_poisoned(),
            "the panic must have poisoned the lock for this test to mean anything"
        );

        // The pool must recover the lock and keep working, not fall through
        // to the cold path forever.
        let (driver, _rx) = InstanceDriver::stub(1, None);
        pool.lock_state().drivers.push(Arc::new(driver));
        assert_eq!(
            pool.lock_state().drivers.len(),
            1,
            "a recovered pool must still hold and serve its warm instances"
        );
        pool.clear();
        assert!(
            pool.lock_state().drivers.is_empty(),
            "a recovered pool must still clear"
        );
    }
}
