//! Warm handler instances for the JetStream delivery loop.
//!
//! Every delivery loop in [`super::subscriber`] built, instantiated, invoked
//! and dropped a store per message. That keeps guest state ephemeral — the
//! default contract — but it also means a guest rebuilds everything it caches
//! in linear memory on every message, and pays instantiation on the critical
//! path of each one. For a request-reply workload the campaign measured that
//! as most of the latency: ~7x between guest languages, dominated by
//! cold-instantiating a multi-megabyte component per request.
//!
//! The engine already has an opt-in for exactly this — `poolSize`,
//! `maxInvocations`, `maxConcurrency` on the component, decoded once into
//! [`InstancePolicy`] — and core and KV deliveries take it: they cross as an
//! [`InstanceJob::Plugin`] and the pool routes them like any other call.
//! **JetStream
//! cannot.** Its call carries a `message-handle` resource, which is an index
//! into one store's resource table, so the argument cannot be built until the
//! store is chosen — and a job the pool hands back has already been pushed
//! into a table it no longer belongs to. Its settle-ack ownership and its
//! rebuild resume point live in the subscription loop for the same reason.
//!
//! So the JetStream loop alone keeps a warm set, honouring the same
//! declaration:
//!
//! * `pool_size` — instances parked between deliveries. A delivery finding
//!   none idle builds a one-shot store exactly as before, so pooling never
//!   adds latency it did not save (the same rule as
//!   [`Dispatch::Saturated`]). When a one-shot completes and the pool has
//!   room, it is parked — that is how the pool fills.
//! * `max_invocations` — an instance is dropped rather than parked once it
//!   has served that many, so guest state (and any message-handle resources a
//!   guest leaked into the store's table) cannot accumulate forever.
//! * `max_concurrency` — **not honoured per instance here.** These are
//!   checkout instances: the proxy call takes `&mut Store`, so one call at a
//!   time each, which is `max_concurrency`'s default. Deliveries beyond the
//!   idle set are served from one-shot stores, so a value above 1 loses no
//!   throughput; it just does not multiply reuse. Core and KV deliveries do
//!   honour it, because the driver holds the store in one long
//!   `run_concurrent` and their calls arrive as tasks on it.
//!
//! A trapped store is poison — it can no longer enter any component instance —
//! so a call that traps drops its store instead of parking it, and the next
//! delivery starts fresh. A handler that merely *returns* an error keeps its
//! instance: the store is healthy, and an error path that recycled the
//! instance would hand a failing-but-hot workload the cold-start cost on every
//! message, exactly when it can least afford it.
//!
//! Everything an instance outliving a call implies is the same here as on the
//! driver path, and a component opts into all of it with `poolSize`: its
//! context is frozen between recycles, and guest tasks spawned-but-not-awaited
//! resume alongside later calls. See [`crate::engine::instance_pool`].
//!
//! [`InstancePolicy`]: crate::engine::InstancePolicy
//! [`InstanceJob`]: crate::engine::instance_driver::InstanceJob
//! [`Dispatch::Saturated`]: crate::engine::instance_pool::Dispatch::Saturated

use std::num::NonZeroUsize;
use std::sync::Mutex;

use crate::engine::InstancePolicy;

/// A parked handler and how many deliveries it has served.
pub(super) struct Warmed<H> {
    pub(super) handler: H,
    invocations: usize,
}

impl<H> Warmed<H> {
    /// Wrap a handler built cold for one delivery, so its completion can offer
    /// it to the pool like any other.
    pub(super) fn fresh(handler: H) -> Self {
        Self {
            handler,
            invocations: 0,
        }
    }
}

/// The warm handlers of one component on one binding.
///
/// Shared by that binding's subscriptions of one handler flavour, so a
/// component subscribed to several subjects serves them all from one warm set
/// — which is what pooling means. A component exporting more than one handler
/// flavour keeps a set per flavour, since their instantiated proxies are
/// different types; in practice a component exports one.
pub(super) struct WarmSet<H> {
    idle: Mutex<Vec<Warmed<H>>>,
    /// Zero when the component (or something linked into its store) did not
    /// opt in: nothing is ever parked.
    pool_size: usize,
    max_invocations: Option<NonZeroUsize>,
}

impl<H> WarmSet<H> {
    pub(super) fn new(policy: InstancePolicy) -> Self {
        let (pool_size, max_invocations) = match policy {
            InstancePolicy::Ephemeral => (0, None),
            InstancePolicy::Warm {
                pool_size,
                max_invocations,
                ..
            } => (pool_size.get(), max_invocations),
        };
        Self {
            idle: Mutex::new(Vec::new()),
            pool_size,
            max_invocations,
        }
    }

    /// Whether this component keeps instances at all, so the bind can say so.
    pub(super) fn keeps_instances(&self) -> bool {
        self.pool_size > 0
    }

    /// Take an idle instance, most recently parked first — the one whose
    /// caches are warmest, and what lets an over-provisioned set go unused
    /// rather than round-robining every instance lukewarm.
    pub(super) fn checkout(&self) -> Option<Warmed<H>> {
        self.idle.lock().ok()?.pop()
    }

    /// Offer a handler back after a completed call.
    ///
    /// Counts the call, then parks unless the component keeps no instances,
    /// this one's invocation budget is spent, or the pool is full — in each
    /// of those cases the handler (and its store) is dropped here. The caller
    /// must *not* offer a store whose call trapped: a trapped store is
    /// permanently unable to enter any component instance, and parking it
    /// would hand the poison to the next delivery.
    pub(super) fn park(&self, mut warmed: Warmed<H>) {
        warmed.invocations = warmed.invocations.saturating_add(1);
        if self.pool_size == 0 {
            return;
        }
        if let Some(limit) = self.max_invocations
            && warmed.invocations >= limit.get()
        {
            return;
        }
        let Ok(mut idle) = self.idle.lock() else {
            return;
        };
        if idle.len() >= self.pool_size {
            return;
        }
        idle.push(warmed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warm(pool_size: usize, max_invocations: usize) -> InstancePolicy {
        InstancePolicy::from_limits(pool_size as i32, max_invocations as i32, 0)
    }

    #[test]
    fn an_ephemeral_component_parks_nothing() {
        let set: WarmSet<&str> = WarmSet::new(InstancePolicy::Ephemeral);
        assert!(!set.keeps_instances());
        set.park(Warmed::fresh("instance"));
        assert!(set.checkout().is_none());
    }

    #[test]
    fn a_parked_instance_is_reused_and_its_calls_are_counted() {
        let set: WarmSet<&str> = WarmSet::new(warm(2, 0));
        set.park(Warmed::fresh("instance"));
        let out = set.checkout().expect("the parked instance");
        assert_eq!(out.handler, "instance");
        assert_eq!(out.invocations, 1);
        assert!(set.checkout().is_none(), "checkout removes it from the set");
    }

    #[test]
    fn the_pool_holds_at_most_pool_size() {
        let set: WarmSet<u32> = WarmSet::new(warm(2, 0));
        set.park(Warmed::fresh(1));
        set.park(Warmed::fresh(2));
        set.park(Warmed::fresh(3)); // dropped: pool full
        assert!(set.checkout().is_some());
        assert!(set.checkout().is_some());
        assert!(set.checkout().is_none());
    }

    /// `max_invocations` is what bounds guest state accumulating in a reused
    /// store — leaked resources included — so it must retire the instance, not
    /// merely count.
    #[test]
    fn an_instance_retires_at_its_invocation_budget() {
        let set: WarmSet<&str> = WarmSet::new(warm(4, 2));
        set.park(Warmed::fresh("instance"));
        let after_one = set.checkout().expect("still under budget");
        set.park(after_one);
        assert!(
            set.checkout().is_none(),
            "the second call spent the budget; the instance must not come back"
        );
    }

    /// Most recently parked first: the warmest instance serves next, and an
    /// over-provisioned pool leaves its excess idle rather than cycling
    /// everything lukewarm.
    #[test]
    fn checkout_is_lifo() {
        let set: WarmSet<u32> = WarmSet::new(warm(3, 0));
        set.park(Warmed::fresh(1));
        set.park(Warmed::fresh(2));
        assert_eq!(set.checkout().map(|w| w.handler), Some(2));
        assert_eq!(set.checkout().map(|w| w.handler), Some(1));
    }
}
