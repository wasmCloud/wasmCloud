//! Plugin-initiated dispatch: a host plugin running work on a workload's own
//! guest code.
//!
//! This is the direction a *push-mode* capability runs in. A plugin that serves
//! an interface waits to be called; a plugin that consumes an external event
//! stream — a broker subscription, a topic partition, a timer — has to call
//! *into* the workload instead, on whichever of its items exports the interface
//! the plugin drives.
//!
//! A plugin resolves one [`DispatchTarget`] per item it will call, once, while
//! the workload resolves ([`HostPlugin::on_workload_resolved`]), and dispatches
//! through it for as long as the workload is up. The target hides which shape
//! the item is, because the two are reached in genuinely different ways:
//!
//!  * a **component** is instantiated per call, unless it keeps instances warm
//!    — in which case the call runs on one of them, alongside whatever that
//!    instance already has in flight, exactly as an inbound HTTP request or a
//!    call from another component does (see the `instance_pool` module).
//!    That is what makes `poolSize`/`maxConcurrency` mean something here: a
//!    burst of dispatches fans out across the warm set rather than paying for a
//!    store apiece.
//!  * a **service** is the workload's one long-lived instance and is never
//!    instantiated again. Its calls are delivered to the instance that is
//!    already running, over the ingress its trigger-service driver serves (see
//!    [`crate::host::trigger_service`]), so they share the state the service
//!    has built up — which is the entire reason to write a handler as a service.
//!
//! The unit of dispatch is a [`GuestCall`]: the plugin's own code, run on the
//! instance the host picked, with the [`Accessor`] that instance's store is
//! being driven under. A plugin therefore keeps using its generated bindings
//! for the call itself — the host decides *where* the call runs, not *what* it
//! is — and carries results back over a channel of its own.
//!
//! [`HostPlugin::on_workload_resolved`]: crate::plugin::HostPlugin::on_workload_resolved

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context as _, bail};
use tokio::sync::{mpsc, oneshot};
use tokio_util::task::AbortOnDropHandle;
use wasmtime::component::{Accessor, AccessorTask, Instance, InstancePre};

use crate::engine::ctx::SharedCtx;
use crate::engine::instance_driver::{InstanceJob, PoolSlot};
use crate::engine::instance_pool::{self, ComponentInstance, Declined, InstancePool};
use crate::engine::workload::ResolvedWorkload;

/// Invocations one of a service's ingresses may have queued before its driver
/// takes them — dispatched calls here, and the HTTP and messaging ingresses
/// built beside them (see `build_trigger_ingresses`). A dispatcher that fills it
/// waits, which is the back-pressure a plugin reading an external stream wants.
pub(crate) const INGRESS_BACKLOG: usize = 256;

/// A future borrowing the accessor a [`GuestCall`] was handed.
pub type GuestCallFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// Work a host plugin runs on a live instance of a workload item.
///
/// The host resolves the instance — a warm one, one built for this call, or the
/// running service — and drives its store; the call itself is the plugin's,
/// made through whatever bindings it generated for the interface:
///
/// ```no_run
/// # use std::sync::Arc;
/// # use wasmtime::component::{Accessor, Instance};
/// # use wash_runtime::engine::ctx::SharedCtx;
/// # use wash_runtime::engine::dispatch::{GuestCall, GuestCallFuture};
/// # struct Records;
/// # struct Handler;
/// # impl Handler {
/// #     fn new(_: &mut impl wasmtime::AsContextMut, _: &Instance) -> anyhow::Result<Self> { todo!() }
/// #     async fn call_handle(&self, _: &Accessor<SharedCtx>, _: Records) -> anyhow::Result<()> { todo!() }
/// # }
/// struct Deliver {
///     records: Records,
///     reply: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
/// }
///
/// impl GuestCall for Deliver {
///     fn call<'a>(
///         self: Box<Self>,
///         accessor: &'a Accessor<SharedCtx>,
///         instance: Instance,
///     ) -> GuestCallFuture<'a> {
///         Box::pin(async move {
///             let handler = accessor.with(|mut access| Handler::new(&mut access, &instance))?;
///             let outcome = handler.call_handle(accessor, self.records).await;
///             let _ = self.reply.send(outcome);
///             Ok(())
///         })
///     }
/// }
/// ```
///
/// # Errors
///
/// The `Err` a call returns is reported to whoever dispatched it, and means the
/// call did not complete: a pooled instance that was serving it is retired
/// rather than left holding guest state no one can account for. A guest that
/// answers with its interface's own error type has *completed* — report that
/// through the plugin's own channel, as above, and return `Ok`.
pub trait GuestCall: Send + 'static {
    /// Run this call on `instance`, under the store `accessor` is driving.
    fn call<'a>(
        self: Box<Self>,
        accessor: &'a Accessor<SharedCtx>,
        instance: Instance,
    ) -> GuestCallFuture<'a>;
}

/// A [`GuestCall`] with the channel its outcome goes back on, as it travels to
/// the instance that will serve it.
///
/// Opaque: a job is minted by [`DispatchTarget::dispatch`] and is only ever
/// handed to the instance that runs it.
pub struct GuestJob {
    call: Box<dyn GuestCall>,
    reply: oneshot::Sender<anyhow::Result<()>>,
}

/// Serves one plugin-dispatched call on an instance: the workload's service, or
/// one warm instance of a pooled component.
///
/// A call that fails retires the pooled instance it ran on. The host cannot know
/// what the plugin's call left behind — a timed-out guest task is still running
/// on that store, and a host-side failure mid-call leaves guest state
/// indeterminate — so the instance drains and its store drops rather than
/// serving anything else. A service's singleton instance is not the pool's to
/// retire, so it keeps serving; a guest *trap* faults its store either way, and
/// the supervisor restarts it.
pub(crate) struct GuestTask {
    pub(crate) instance: Instance,
    pub(crate) job: GuestJob,
    /// This call's tether to a pooled instance. `None` for a service.
    pub(crate) pool_slot: Option<PoolSlot>,
}

impl AccessorTask<SharedCtx> for GuestTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        let GuestTask {
            instance,
            job,
            pool_slot,
        } = self;
        let GuestJob { call, reply } = job;
        let outcome = call.call(accessor, instance).await;
        if let Err(e) = &outcome
            && let Some(slot) = &pool_slot
        {
            tracing::warn!(
                err = ?e,
                "dispatched call failed; retiring the instance that served it"
            );
            slot.retire_instance();
        }
        // The dispatcher may have gone; the call still ran, because a guest
        // subtask cannot be cancelled from the host.
        let _ = reply.send(outcome);
        Ok(())
    }
}

/// The ingress a workload's service serves plugin-dispatched calls on.
///
/// One per workload, shared by every plugin that dispatches to its service, and
/// rebuilt per incarnation: a restarted service is a new instance with a new
/// channel, and the sender held here is swapped for it, exactly as the
/// host-invoked ingresses beside it are re-registered.
///
/// The channel is created when a plugin *claims* the service, during resolve,
/// rather than when the service starts — so a plugin that begins dispatching
/// the moment it is told the workload resolved queues those calls instead of
/// finding nothing to send them to.
#[derive(Default)]
pub(crate) struct ServiceCalls {
    state: Mutex<ServiceCallState>,
}

#[derive(Default)]
struct ServiceCallState {
    /// Sender for the incarnation now running, or for the queue waiting on the
    /// first one. `None` until a plugin claims the service.
    tx: Option<mpsc::Sender<GuestJob>>,
    /// The receiver the next incarnation takes. Holds the claim-time queue
    /// until the service first starts.
    rx: Option<mpsc::Receiver<GuestJob>>,
    /// Live claims. The ingress exists while there is at least one, so a claim
    /// taken speculatively — a host component plugin resolving a route it may
    /// then discard in favour of a component's — leaves nothing behind when it
    /// goes.
    claims: usize,
    /// Whether the service has started. A claim after that cannot be served:
    /// its ingress would have to be added to a driver that is already running.
    started: bool,
}

impl ServiceCalls {
    fn lock(&self) -> std::sync::MutexGuard<'_, ServiceCallState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claim the service as a dispatch target, creating its ingress if this is
    /// the first claim. Several claims share the one ingress, which lasts as
    /// long as the last of them.
    pub(crate) fn claim(self: &Arc<Self>) -> anyhow::Result<ServiceClaim> {
        let mut state = self.lock();
        if state.started {
            bail!(
                "the workload's service is already running, so it cannot be claimed as a \
                 dispatch target; resolve the target while the workload resolves"
            );
        }
        if state.tx.is_none() {
            let (tx, rx) = mpsc::channel(INGRESS_BACKLOG);
            state.tx = Some(tx);
            state.rx = Some(rx);
        }
        state.claims += 1;
        Ok(ServiceClaim {
            calls: Arc::clone(self),
            released: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Give one claim back. The ingress goes with the last of them, but only
    /// while the service has yet to start: once it is running with one, closing
    /// the channel would end its driver, and a plugin letting go of a target is
    /// no reason to stop the service.
    fn release_claim(&self) {
        let mut state = self.lock();
        state.claims = state.claims.saturating_sub(1);
        if state.claims == 0 && !state.started {
            state.tx = None;
            state.rx = None;
        }
    }

    /// Whether any plugin claimed this service, and so whether it has to be run
    /// with an ingress to serve them.
    pub(crate) fn claimed(&self) -> bool {
        self.lock().tx.is_some()
    }

    /// Record that the service is starting, whichever way it runs.
    ///
    /// What a claim after this point would produce is a channel no driver ever
    /// serves — a dispatch into it would wait forever — so this is what makes
    /// such a claim an error instead.
    pub(crate) fn mark_started(&self) {
        self.lock().started = true;
    }

    /// The receiver an incarnation of the service serves its dispatched calls
    /// on, or `None` when nothing claimed it.
    ///
    /// The first incarnation inherits the channel the claim created, along with
    /// whatever queued on it before the service started. Each later one gets a
    /// fresh channel, and the sender dispatchers hold is swapped for it — calls
    /// still queued on the faulted incarnation's channel end with it, as an
    /// inbound request to a restarting service does.
    pub(crate) fn next_incarnation(&self) -> Option<mpsc::Receiver<GuestJob>> {
        let mut state = self.lock();
        state.tx.as_ref()?;
        // The claim's receiver is there exactly once, which is what marks this
        // as the first incarnation.
        if let Some(rx) = state.rx.take() {
            return Some(rx);
        }
        let (tx, rx) = mpsc::channel(INGRESS_BACKLOG);
        state.tx = Some(tx);
        Some(rx)
    }

    /// Stop accepting calls: the workload is going away, so a dispatch that
    /// would otherwise wait on a reply that can never come fails instead.
    pub(crate) fn shutdown(&self) {
        let mut state = self.lock();
        state.tx = None;
        state.rx = None;
    }

    fn sender(&self) -> Option<mpsc::Sender<GuestJob>> {
        self.lock().tx.clone()
    }
}

/// One live claim on a service's dispatch ingress: proof that the service will
/// be run with one, and what keeps it in place.
///
/// Held by whoever may dispatch — a [`DispatchTarget`], or the route a host
/// component plugin resolved — and released when they let go. A route that
/// loses to a component's is released explicitly rather than left to its drop,
/// so the service's ingress reflects the routing decision at the moment it is
/// made, not whenever the last handle to the losing route goes away.
pub(crate) struct ServiceClaim {
    calls: Arc<ServiceCalls>,
    released: std::sync::atomic::AtomicBool,
}

impl ServiceClaim {
    /// The ingress this claim holds open.
    pub(crate) fn calls(&self) -> &Arc<ServiceCalls> {
        &self.calls
    }

    /// Give this claim back now rather than when it drops. Idempotent, so the
    /// drop that follows does nothing.
    pub(crate) fn release(&self) {
        if !self
            .released
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            self.calls.release_claim();
        }
    }
}

impl Clone for ServiceClaim {
    /// A copy of a claim is a claim of its own, so the ingress outlives whichever
    /// of them is dropped first. Never refused: an existing claim is proof the
    /// service was claimed in time, whatever it has done since.
    fn clone(&self) -> Self {
        self.calls.lock().claims += 1;
        Self {
            calls: Arc::clone(&self.calls),
            released: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl Drop for ServiceClaim {
    fn drop(&mut self) {
        self.release();
    }
}

/// One workload item a host plugin can dispatch [`GuestCall`]s to, resolved
/// once from [`ResolvedWorkload::dispatch_target`].
///
/// Cheap to clone and safe to hold for the life of the binding. It pins *which*
/// item calls go to, not the instance that serves them: a component's warm set
/// and a service's incarnations both turn over underneath it.
///
/// The workload is behind an [`Arc`] because cloning a [`ResolvedWorkload`] is
/// not cheap — it copies the service's `Linker` by value, string pool and all —
/// and a target is expected to be cloned per dispatch.
#[derive(Clone)]
pub struct DispatchTarget {
    workload: Arc<ResolvedWorkload>,
    item: TargetItem,
}

#[derive(Clone)]
enum TargetItem {
    /// A component: instantiated per call, or served on a warm instance.
    Component {
        id: Arc<str>,
        /// Resolved once, with the target, rather than per call: pre-linking a
        /// component against its linker checks every import, which is exactly
        /// the work `InstancePre` exists to do ahead of time. The HTTP
        /// entrypoint captures one the same way, at the same point.
        pre: InstancePre<SharedCtx>,
        /// The warm set this component's calls run on, or `None` when each gets
        /// a store of its own. Read once, with `pre` and under the same lock:
        /// which pool serves a component is settled when the workload resolves,
        /// so a per-call re-read would cost a lock to learn the same answer —
        /// and would disagree with the `InstancePre` beside it, which is pinned
        /// here either way.
        pool: Option<Arc<InstancePool>>,
    },
    /// The workload's long-lived service, reached through the ingress this
    /// claim holds open. Behind an `Arc` so cloning a target is one atomic
    /// rather than a lock on the ingress to count one more claim.
    Service(Arc<ServiceClaim>),
}

impl std::fmt::Debug for DispatchTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("DispatchTarget");
        s.field("workload_id", &self.workload.id());
        match &self.item {
            TargetItem::Component { id, .. } => s.field("component", id),
            TargetItem::Service(_) => s.field("service", &self.workload.service_id()),
        };
        s.finish()
    }
}

impl DispatchTarget {
    /// Resolve `item_id` — a component of `workload`, or its service — as a
    /// dispatch target. See [`ResolvedWorkload::dispatch_target`].
    pub(crate) async fn resolve(
        workload: &ResolvedWorkload,
        item_id: &str,
    ) -> anyhow::Result<Self> {
        let item = if workload.is_service_item(item_id) {
            // Asked for by name, so a service that can never serve a dispatched
            // call is an error here rather than something to skip past.
            TargetItem::Service(Arc::new(workload.claim_service_dispatch()?))
        } else {
            let (id, pre, pool) = workload.component_dispatch(item_id).await?;
            TargetItem::Component { id, pre, pool }
        };
        Ok(Self {
            // Copied once per target: cloning a `ResolvedWorkload` copies the
            // service's `Linker` by value, so it is worth paying here rather
            // than on every clone of the target.
            workload: Arc::new(workload.clone()),
            item,
        })
    }

    /// The workload this target belongs to.
    pub fn workload(&self) -> &ResolvedWorkload {
        &self.workload
    }

    /// Run `call` on a live instance of this item, and wait for it.
    ///
    /// Where it runs is the host's to choose (see the [module docs]); what it
    /// does is the caller's. The `Err` a call returns comes back here, as does a
    /// failure to reach an instance at all — a component that will not
    /// instantiate, or a service that is not running.
    ///
    /// # How long it may take
    ///
    /// No host timeout bounds the call: the host cannot know what the plugin
    /// asked the guest to do, and a batch handler that takes minutes is the
    /// point of this interface. A caller that wants a deadline imposes its own
    /// by dropping this future, which reclaims a store built for the call alone.
    /// It cannot end guest work already running on a *shared* instance, though —
    /// a warm one or the service — because a guest subtask cannot be cancelled
    /// from the host: that call runs to completion on the instance, with nowhere
    /// left to report its outcome.
    ///
    /// [module docs]: self
    pub async fn dispatch(&self, call: impl GuestCall) -> anyhow::Result<()> {
        // A target outlives the workload it names: a plugin holds one until it
        // is unbound, and teardown begins before that. Without this a dispatch
        // racing a stop would build — and, for a pooled component, *park* — a
        // fresh instance in a workload the host has already stopped.
        if self.workload.released() {
            bail!(
                "workload '{}' has stopped and takes no more dispatched calls",
                self.workload.id()
            );
        }
        let call = Box::new(call);
        match &self.item {
            TargetItem::Component { id, pre, pool } => {
                dispatch_to_component(&self.workload, id, pre, pool.as_ref(), call).await
            }
            TargetItem::Service(claim) => dispatch_to_service(claim.calls(), call).await,
        }
    }
}

/// Deliver a call to the workload's running service, and wait for it.
pub(crate) async fn dispatch_to_service(
    calls: &ServiceCalls,
    call: Box<dyn GuestCall>,
) -> anyhow::Result<()> {
    let tx = calls
        .sender()
        .context("the workload's service is not accepting dispatched calls")?;
    let (reply, reply_rx) = oneshot::channel();
    tx.send(GuestJob { call, reply })
        .await
        .map_err(|_| anyhow::anyhow!("the workload's service is no longer running"))?;
    reply_rx
        .await
        .map_err(|_| anyhow::anyhow!("the workload's service dropped the dispatched call"))?
}

/// Run a call on `component_id`: on one of its warm instances when it keeps
/// any, otherwise — and when every warm instance is busy and the pool is full —
/// on a store built, instantiated and dropped for this call alone.
async fn dispatch_to_component(
    workload: &ResolvedWorkload,
    component_id: &str,
    pre: &InstancePre<SharedCtx>,
    pool: Option<&Arc<InstancePool>>,
    call: Box<dyn GuestCall>,
) -> anyhow::Result<()> {
    // An instance built for a pool that then declined the call: the store of
    // its own below is that instance, rather than a second one beside it.
    let mut reclaimed = None;
    let call = if let Some(pool) = pool {
        let (reply, reply_rx) = oneshot::channel();
        let job = InstanceJob::Guest(GuestJob { call, reply });
        let outcome =
            instance_pool::offer_or_install(pool, pre, job, || workload.new_store(component_id))
                .await?;
        match outcome {
            Ok(()) => {
                return reply_rx
                    .await
                    .map_err(|_| anyhow::anyhow!("pooled instance dropped the dispatched call"))?;
            }
            // Every warm instance was busy; run it in a store of its own.
            Err(Declined {
                job: InstanceJob::Guest(job),
                instance,
            }) => {
                tracing::debug!(
                    component_id,
                    "warm instances saturated; dispatching to a store of its own"
                );
                reclaimed = instance;
                job.call
            }
            // A job comes back as the variant it went in as, so this is
            // unreachable — but not worth a panic on a dispatch path.
            Err(_) => {
                debug_assert!(false, "a dispatched job cannot come back as another kind");
                bail!("instance pool returned another kind of job for a dispatched call");
            }
        }
    } else {
        call
    };

    let ComponentInstance {
        mut store,
        instance,
    } = match reclaimed {
        Some(built) => built,
        None => {
            let mut store = workload.new_store(component_id).await?;
            let instance = pre.instantiate_async(&mut store).await?;
            ComponentInstance { store, instance }
        }
    };
    // The store travels into the task, so a dispatcher cancelled mid-call drops
    // it with the task rather than leaving it running.
    let mut task = AbortOnDropHandle::new(tokio::spawn(async move {
        store
            .run_concurrent(async move |accessor| call.call(accessor, instance).await)
            .await
            .map_err(|e| anyhow::anyhow!("dispatched call store faulted: {e:#}"))?
    }));
    (&mut task).await.context("dispatched call task failed")?
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::ServiceCalls;

    /// A service nothing claimed runs without an ingress, and has no sender for
    /// a call to be delivered on.
    #[test]
    fn unclaimed_service_has_no_ingress() {
        let calls = Arc::new(ServiceCalls::default());
        assert!(!calls.claimed());
        assert!(calls.next_incarnation().is_none());
        assert!(calls.sender().is_none());
    }

    /// Several plugins claiming the one service share its ingress, and the
    /// first incarnation serves the very channel the claim created — so a call
    /// dispatched before the service started is still waiting on it.
    #[test]
    fn claims_share_one_ingress_and_the_first_incarnation_takes_it() {
        let calls = Arc::new(ServiceCalls::default());
        let _first_claim = calls.claim().expect("first claim");
        let queued = calls.sender().expect("a claim opens the ingress");
        let _second_claim = calls.claim().expect("second claim");
        assert!(
            queued.same_channel(&calls.sender().expect("still open")),
            "a second claim must not replace the channel the first is dispatching on"
        );

        calls.mark_started();
        let first = calls.next_incarnation().expect("claimed, so it has one");
        assert!(
            queued.same_channel(&calls.sender().expect("still open")),
            "the first incarnation serves the claim-time channel, queue and all"
        );
        drop(first);
    }

    /// A claim given back before the service starts takes its ingress with it —
    /// but only the last one does, so a plugin that resolved a service route and
    /// then routed to a component instead cannot strand the service with an
    /// ingress nothing sends on, nor take one another plugin still wants.
    #[test]
    fn the_last_claim_released_takes_the_ingress_with_it() {
        let calls = Arc::new(ServiceCalls::default());
        let one = calls.claim().expect("first claim");
        let two = calls.claim().expect("second claim");

        one.release();
        assert!(
            calls.claimed(),
            "a claim released while another is held must leave the ingress in place"
        );
        // Releasing twice is the same as releasing once: the drop below must not
        // take the ingress from a claim that is still held.
        one.release();
        assert!(calls.claimed(), "a repeated release must not double-count");

        drop(two);
        assert!(
            !calls.claimed(),
            "the last claim released takes the ingress with it, so the service is not \
             run with one"
        );
        assert!(calls.next_incarnation().is_none());
    }

    /// Once the service is running with an ingress, a claim going away does not
    /// take it: closing the channel would end the driver, and a plugin letting
    /// go of a target is no reason to stop the service.
    #[test]
    fn releasing_a_claim_after_the_service_started_keeps_the_ingress() {
        let calls = Arc::new(ServiceCalls::default());
        let claim = calls.claim().expect("claim");
        calls.mark_started();
        let _rx = calls.next_incarnation().expect("first incarnation");

        drop(claim);
        assert!(
            calls.claimed(),
            "a running service keeps the ingress it was started with"
        );
    }

    /// Each restart gets a fresh channel, and dispatchers are swapped onto it.
    #[test]
    fn a_restart_swaps_the_channel() {
        let calls = Arc::new(ServiceCalls::default());
        let _claim = calls.claim().expect("claim");
        calls.mark_started();
        let _first = calls.next_incarnation().expect("first incarnation");
        let before = calls.sender().expect("open");

        let _second = calls.next_incarnation().expect("restarted incarnation");
        let after = calls.sender().expect("open");
        assert!(
            !before.same_channel(&after),
            "a restarted service serves a new channel, so dispatchers must be moved to it"
        );
    }

    /// A claim once the service is running is refused: its ingress could only be
    /// added to a driver that is already serving, so a call dispatched into it
    /// would wait forever.
    #[test]
    fn a_claim_after_the_service_started_is_refused() {
        let calls = Arc::new(ServiceCalls::default());
        calls.mark_started();
        assert!(calls.claim().is_err());
        assert!(!calls.claimed());
    }

    /// Shutdown closes the ingress, so a later dispatch fails instead of waiting
    /// on a driver that is being torn down.
    #[test]
    fn shutdown_closes_the_ingress() {
        let calls = Arc::new(ServiceCalls::default());
        let _claim = calls.claim().expect("claim");
        calls.mark_started();
        let _rx = calls.next_incarnation().expect("first incarnation");
        calls.shutdown();
        assert!(calls.sender().is_none());
        assert!(!calls.claimed());
    }
}
