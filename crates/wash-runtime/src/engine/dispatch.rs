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
//!    That is what makes `poolSize`, `maxInvocations` and `maxConcurrency` mean
//!    something here: a burst of dispatches fans out across the warm set rather
//!    than paying for a store apiece, and an instance that has served its
//!    budget is replaced under the plugin without it noticing.
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
//! Everything a shared instance owes a call it did not build is the host's, and
//! is why a plugin dispatching here behaves better than one keeping a warm set
//! of its own: the epoch deadline is re-armed so it measures the call rather
//! than the instance's whole life, the call is registered with its store's
//! abandonment set for as long as it runs, it is bounded by
//! [`GuestCall::deadline`], an instance it leaves in an indeterminate state is
//! retired, and its guest execution is recorded under the plugin that drove it.
//!
//! Dropping a dispatch future or [`DispatchHandle`] cancels its call. Wasmtime
//! cannot stop one guest task without dropping its store, so behavior depends
//! on placement (bytecodealliance/wasmtime#11833). Use
//! [`CancelHandle::quiesced`] to wait until the call can no longer run.
//!
//! [`HostPlugin::on_workload_resolved`]: crate::plugin::HostPlugin::on_workload_resolved

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use anyhow::bail;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use wasmtime::component::{Accessor, AccessorTask, Instance, InstancePre};

use crate::engine::abandon::{AbandonFlag, DispatchedCall};
use crate::engine::ctx::SharedCtx;
use crate::engine::instance_driver::{InstanceJob, InvocationSample, PoolSlot};
use crate::engine::instance_pool::{ComponentInstance, Declined, Dispatch, InstancePool};
use crate::engine::workload::ResolvedWorkload;

/// Invocations one of a service's ingresses may have queued before its driver
/// takes them — dispatched calls here, and the HTTP and messaging ingresses
/// built beside them (see `build_trigger_ingresses`).
///
/// This bounds the queue, not the work: the serve loop takes a job and spawns it
/// straight away, so the channel only fills while the driver itself is stalled.
/// What bounds concurrent calls on a service's one instance is
/// [`MAX_INFLIGHT_GUEST_CALLS`].
///
/// [`MAX_INFLIGHT_GUEST_CALLS`]: crate::host::trigger_service::MAX_INFLIGHT_GUEST_CALLS
pub(crate) const INGRESS_BACKLOG: usize = 256;

/// A future borrowing the accessor a [`GuestCall`] was handed.
pub type GuestCallFuture<'a> = Pin<Box<dyn Future<Output = GuestCallOutcome> + Send + 'a>>;

/// What a [`GuestCall`] reports back to the host.
///
///  * `Ok(None)` — the guest ran and answered. Whatever it answered is the
///    plugin's to interpret and to report on its own channel.
///  * `Ok(Some(label))` — the guest ran and answered with a failure the plugin
///    counts as one: a handler returning its interface's own error. `label` is
///    what the call is recorded under. It must be short, bounded, and not
///    supplied by a caller.
///  * `Err` — the call did not complete, so a pooled instance that was serving
///    it is retired rather than left holding guest state no one can account
///    for.
pub type GuestCallOutcome = anyhow::Result<Option<&'static str>>;

/// Work a host plugin runs on a live instance of a workload item.
///
/// The host resolves the instance — a warm one, one built for this call, or the
/// running service — drives its store, and owns everything a shared instance
/// needs around the call: the epoch deadline is re-armed so it measures this
/// call rather than the instance's whole life, the call is registered with the
/// store's abandonment set for as long as it runs, it is bounded by
/// [`Self::deadline`], and its guest execution is recorded. The call itself is
/// the plugin's, made through whatever bindings it generated for the interface:
///
/// ```no_run
/// # use wasmtime::component::{Accessor, Instance};
/// # use wash_runtime::engine::ctx::SharedCtx;
/// # use wash_runtime::engine::dispatch::{GuestCall, GuestCallFuture};
/// # struct Records;
/// # struct Handler;
/// # impl Handler {
/// #     fn new(_: &mut impl wasmtime::AsContextMut, _: &Instance) -> anyhow::Result<Self> { todo!() }
/// #     async fn call_handle(&self, _: &Accessor<SharedCtx>, _: Records) -> anyhow::Result<Result<(), String>> { todo!() }
/// # }
/// struct Deliver {
///     records: Records,
///     reply: tokio::sync::oneshot::Sender<anyhow::Result<Result<(), String>>>,
/// }
///
/// impl GuestCall for Deliver {
///     fn describe(&self) -> &str {
///         "acme:events/handler#handle"
///     }
///
///     fn call<'a>(
///         self: Box<Self>,
///         accessor: &'a Accessor<SharedCtx>,
///         instance: Instance,
///     ) -> GuestCallFuture<'a> {
///         Box::pin(async move {
///             let handler = accessor.with(|mut access| Handler::new(&mut access, &instance))?;
///             let answered = handler.call_handle(accessor, self.records).await?;
///             let refused = answered.is_err().then_some("handler");
///             let _ = self.reply.send(Ok(answered));
///             Ok(refused)
///         })
///     }
/// }
/// ```
pub trait GuestCall: Send + 'static {
    /// Names this call in the host's log lines, and is the `operation` its
    /// guest execution is recorded under.
    ///
    /// The WIT export being invoked, so it is bounded by the interface set a
    /// component declares — one metric series per export, never one per
    /// message.
    fn describe(&self) -> &str;

    /// How long the host waits for the guest before it stops wanting the
    /// result and retires the instance serving it.
    ///
    /// The default is the host's own ephemeral-call timeout, which is generous
    /// enough for a batch handler. It exists at all because a guest subtask
    /// cannot be cancelled from the host: on a *shared* instance — a warm one,
    /// or the service — a call that never returns would otherwise hold its
    /// in-flight slot for the life of the workload, and retiring the instance
    /// is what ends it.
    fn deadline(&self) -> std::time::Duration {
        crate::timeouts::ephemeral_call()
    }

    /// Run this call on `instance`, under the store `accessor` is driving.
    fn call<'a>(
        self: Box<Self>,
        accessor: &'a Accessor<SharedCtx>,
        instance: Instance,
    ) -> GuestCallFuture<'a>;
}

/// A queued [`GuestCall`] with its reply and control state.
pub struct GuestJob {
    call: Box<dyn GuestCall>,
    reply: oneshot::Sender<anyhow::Result<()>>,
    /// Built where the target was resolved: the identity a dispatched call is
    /// measured under cannot change under a resolved workload, and resolving it
    /// costs a read lock.
    attributes: Arc<[opentelemetry::KeyValue]>,
    /// Shared cancellation and quiescence state.
    control: SettleOnDrop,
}

impl GuestJob {
    /// Mint a job for a dispatcher that runs the pool dance itself rather than
    /// through a [`DispatchTarget`] — the `wasmcloud:nats` subscriber, whose
    /// deliveries carry a cancellation of their own.
    ///
    /// `abandoned` comes from the [`DispatchedCall`] enforcing this job's
    /// deadline, which is what makes the field proof that some dispatcher does.
    #[cfg(any(feature = "wasmcloud-nats", test))]
    pub(crate) fn new(
        call: Box<dyn GuestCall>,
        reply: oneshot::Sender<anyhow::Result<()>>,
        abandoned: Arc<AbandonFlag>,
        attributes: Arc<[opentelemetry::KeyValue]>,
    ) -> Self {
        Self::with_quiescence(call, reply, abandoned, attributes, false)
    }

    fn with_quiescence(
        call: Box<dyn GuestCall>,
        reply: oneshot::Sender<anyhow::Result<()>>,
        abandoned: Arc<AbandonFlag>,
        attributes: Arc<[opentelemetry::KeyValue]>,
        track_quiescence: bool,
    ) -> Self {
        Self {
            call,
            reply,
            attributes,
            control: SettleOnDrop(Arc::new(CallControl::new(abandoned, track_quiescence))),
        }
    }

    /// Turn this job away without running it, and tell its dispatcher why.
    ///
    /// For an ingress at its ceiling: a dispatcher waiting on a call the host
    /// will not admit has to be told, or it waits out the call's whole deadline
    /// for a reply that was never coming.
    pub(crate) fn refuse(self, err: anyhow::Error) {
        let _ = self.reply.send(Err(err));
    }

    /// Run this job on an instance in a store built for it alone, and answer
    /// what it did.
    ///
    /// The outcome is returned rather than sent on the job's reply channel: the
    /// dispatcher is right here awaiting this future, so there is nothing to
    /// deliver it to. The store is dropped with the call, so there is no pooled
    /// instance to retire either — what the call leaves behind goes with it.
    #[cfg(feature = "wasmcloud-nats")]
    pub(crate) async fn run_on_store(
        self,
        store: &mut wasmtime::Store<SharedCtx>,
        instance: Instance,
    ) -> anyhow::Result<()> {
        let GuestJob {
            call,
            reply: _,
            attributes,
            control,
        } = self;
        serve_alone(store, instance, call, attributes, &control.0).await
    }

    /// Runs the job on a dedicated store and sends its outcome.
    async fn run_on_own_store(self, mut store: wasmtime::Store<SharedCtx>, instance: Instance) {
        let GuestJob {
            call,
            reply,
            attributes,
            control,
        } = self;
        let outcome = serve_alone(&mut store, instance, call, attributes, &control.0).await;
        let _ = reply.send(outcome);
    }

    fn cancel_handle(&self) -> CancelHandle {
        CancelHandle(Arc::clone(&self.control.0))
    }
}

/// Runs one call on a dedicated store.
async fn serve_alone(
    store: &mut wasmtime::Store<SharedCtx>,
    instance: Instance,
    call: Box<dyn GuestCall>,
    attributes: Arc<[opentelemetry::KeyValue]>,
    control: &CallControl,
) -> anyhow::Result<()> {
    store
        .run_concurrent(async move |accessor| {
            serve(
                accessor,
                instance,
                call,
                attributes,
                control,
                Placement::OwnStore,
            )
            .await
        })
        .await
        .map_err(|e| anyhow::anyhow!("dispatched call store faulted: {e:#}"))?
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
    pub(crate) placement: Placement,
}

impl AccessorTask<SharedCtx> for GuestTask {
    async fn run(self, accessor: &Accessor<SharedCtx>) -> wasmtime::Result<()> {
        let GuestTask {
            instance,
            job,
            placement,
        } = self;
        let GuestJob {
            call,
            reply,
            attributes,
            control,
        } = job;
        let outcome = serve(accessor, instance, call, attributes, &control.0, placement).await;
        // A dropped dispatcher closes the channel.
        let _ = reply.send(outcome);
        Ok(())
    }
}

/// Placement determines how a running call can stop.
pub(crate) enum Placement {
    /// A warm instance that is retired to stop guest work.
    Pooled(PoolSlot),
    /// A service that must let guest work finish.
    Service,
    /// A dedicated store dropped with the call.
    OwnStore,
}

impl Placement {
    /// Retires a pooled instance.
    fn retire(&self) {
        if let Placement::Pooled(slot) = self {
            slot.retire_instance();
        }
    }
}

/// How the guest side of a served call ended.
enum Ended {
    Returned(GuestCallOutcome),
    TimedOut,
    Stopped(Stop),
}

impl From<Result<GuestCallOutcome, tokio::time::error::Elapsed>> for Ended {
    fn from(ran: Result<GuestCallOutcome, tokio::time::error::Elapsed>) -> Self {
        match ran {
            Ok(outcome) => Self::Returned(outcome),
            Err(_) => Self::TimedOut,
        }
    }
}

/// Run one dispatched call on `instance`, under everything a store owes a call
/// it did not build itself: the epoch deadline re-armed so it measures this
/// call rather than the instance's whole life, the call registered with the
/// store's abandonment set for as long as it runs, its own deadline, and the
/// record of what it cost.
///
/// Canceled calls follow the rules for their [`Placement`].
async fn serve(
    accessor: &Accessor<SharedCtx>,
    instance: Instance,
    call: Box<dyn GuestCall>,
    attributes: Arc<[opentelemetry::KeyValue]>,
    control: &CallControl,
    placement: Placement,
) -> anyhow::Result<()> {
    let what = Arc::<str>::from(call.describe());
    let deadline = call.deadline();

    // Record the store whose drop marks quiescence.
    let started = accessor.with(|mut access| {
        let store_dropped = control
            .tracks_quiescence()
            .then(|| access.get().dropped.token());
        if !control.begin(store_dropped) {
            return None;
        }
        crate::engine::abandon::rearm_for_call(&mut access);
        Some((
            Arc::clone(&access.get().abandoned),
            Arc::clone(&access.get().executed),
        ))
    });
    let Some((calls, executed)) = started else {
        // Preserve the reason that prevented the call from starting.
        return Err(match control.stopped() {
            Some(Stop::Expired) => missed_deadline(),
            Some(Stop::Cancelled) | None => DispatchCancelled.into(),
        });
    };
    let mut sample = InvocationSample::start(&executed, attributes);

    // This bound ends the wait of a dispatcher that is still there; keeping a
    // slow guest out of the epoch callback's reach is `watch_until_abandoned`'s
    // job.
    let running = tokio::time::timeout(
        deadline,
        crate::engine::abandon::watch_until_abandoned(
            &calls,
            Arc::clone(&control.abandoned),
            call.call(accessor, instance),
        ),
    );
    let ended = match &placement {
        // A service call must run to completion.
        Placement::Service => Ended::from(running.await),
        Placement::Pooled(_) | Placement::OwnStore => tokio::select! {
            biased;
            ran = running => Ended::from(ran),
            // The signal fires after the stop reason is stored.
            () = control.cancel.wait() => {
                Ended::Stopped(control.stopped().unwrap_or(Stop::Cancelled))
            }
        },
    };

    match ended {
        Ended::Returned(Ok(refused)) => {
            control.finish();
            if let Some(label) = refused {
                sample.failed(label);
            }
            Ok(())
        }
        // A failed call may leave guest work running until the store drops.
        Ended::Returned(Err(e)) => {
            sample.failed("trap");
            tracing::warn!(
                err = ?e,
                what = %what,
                "dispatched call failed; retiring the instance that served it"
            );
            placement.retire();
            Err(e)
        }
        // A guest subtask cannot be cancelled from the host, so the timed out
        // work is still running on this store. Retiring the instance is what
        // ends it: the driver stops admitting, drains, ends its run loop, and
        // the store's teardown takes the stalled work with it.
        Ended::TimedOut => {
            sample.failed("timeout");
            tracing::warn!(
                what = %what,
                ?deadline,
                "dispatched call did not return within its deadline; retiring the instance \
                 that served it"
            );
            placement.retire();
            Err(anyhow::anyhow!(
                "dispatched call '{what}' did not return within {deadline:?}"
            ))
        }
        Ended::Stopped(stop) => {
            let (label, err) = match stop {
                Stop::Cancelled => ("cancelled", DispatchCancelled.into()),
                Stop::Expired => ("timeout", missed_deadline()),
            };
            sample.failed(label);
            tracing::debug!(
                what = %what,
                stop = label,
                "dispatched call stopped while running"
            );
            placement.retire();
            Err(err)
        }
    }
}

/// The error returned for a canceled dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchCancelled;

impl std::fmt::Display for DispatchCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the dispatched call was cancelled")
    }
}

impl std::error::Error for DispatchCancelled {}

/// Why a call stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Cancelled,
    Expired,
}

/// A call's progress and stop reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallState {
    Pending,
    Running,
    RunningStopped(Stop),
    Finished(Option<Stop>),
    Stopped(Stop),
}

impl CallState {
    fn stopped(self) -> Option<Stop> {
        match self {
            CallState::RunningStopped(stop) | CallState::Stopped(stop) => Some(stop),
            CallState::Finished(stop) => stop,
            CallState::Pending | CallState::Running => None,
        }
    }

    fn may_run(self) -> bool {
        match self {
            CallState::Running | CallState::RunningStopped(_) => true,
            CallState::Pending | CallState::Finished(_) | CallState::Stopped(_) => false,
        }
    }

    const fn bits(self) -> u8 {
        match self {
            CallState::Pending => 0,
            CallState::Running => 1,
            CallState::RunningStopped(Stop::Cancelled) => 2,
            CallState::RunningStopped(Stop::Expired) => 3,
            CallState::Finished(None) => 4,
            CallState::Finished(Some(Stop::Cancelled)) => 5,
            CallState::Finished(Some(Stop::Expired)) => 6,
            CallState::Stopped(Stop::Cancelled) => 7,
            CallState::Stopped(Stop::Expired) => 8,
        }
    }

    fn from_bits(bits: u8) -> Self {
        match bits {
            0 => CallState::Pending,
            1 => CallState::Running,
            2 => CallState::RunningStopped(Stop::Cancelled),
            3 => CallState::RunningStopped(Stop::Expired),
            4 => CallState::Finished(None),
            5 => CallState::Finished(Some(Stop::Cancelled)),
            6 => CallState::Finished(Some(Stop::Expired)),
            7 => CallState::Stopped(Stop::Cancelled),
            8 => CallState::Stopped(Stop::Expired),
            // Unknown states cannot prove quiescence.
            _ => {
                debug_assert!(false, "a call state is one of its own encodings");
                CallState::Running
            }
        }
    }
}

/// Stores progress and the stop reason in one atomic value.
struct AtomicCallState(AtomicU8);

impl AtomicCallState {
    fn new(state: CallState) -> Self {
        Self(AtomicU8::new(state.bits()))
    }

    fn load(&self) -> CallState {
        CallState::from_bits(self.0.load(Ordering::SeqCst))
    }

    /// Applies one atomic state transition.
    fn update(
        &self,
        mut next: impl FnMut(CallState) -> Option<CallState>,
    ) -> Result<CallState, CallState> {
        self.0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |bits| {
                next(CallState::from_bits(bits)).map(CallState::bits)
            })
            .map(CallState::from_bits)
            .map_err(CallState::from_bits)
    }
}

/// An allocation-free notification that remains fired.
#[derive(Default)]
struct Signal {
    fired: AtomicBool,
    notify: tokio::sync::Notify,
}

impl Signal {
    fn fire(&self) {
        if !self.fired.swap(true, Ordering::Release) {
            self.notify.notify_waiters();
        }
    }

    fn is_fired(&self) -> bool {
        self.fired.load(Ordering::Acquire)
    }

    async fn wait(&self) {
        loop {
            if self.is_fired() {
                return;
            }
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_fired() {
                return;
            }
            notified.await;
        }
    }
}

/// Quiescence state for calls started with [`DispatchTarget::start`].
#[derive(Default)]
struct Quiescence {
    settled: Signal,
    store_dropped: OnceLock<CancellationToken>,
}

/// State shared by a call and its handles.
struct CallControl {
    /// Wakes the task serving or preparing the call.
    cancel: Signal,
    /// Atomic progress and stop reason.
    state: AtomicCallState,
    /// Tracks when the call can no longer run.
    quiescence: Option<Quiescence>,
    /// Stops compiled guest code through the store's epoch callback.
    abandoned: Arc<AbandonFlag>,
}

impl CallControl {
    fn new(abandoned: Arc<AbandonFlag>, track_quiescence: bool) -> Self {
        Self {
            cancel: Signal::default(),
            state: AtomicCallState::new(CallState::Pending),
            quiescence: track_quiescence.then(Quiescence::default),
            abandoned,
        }
    }

    fn tracks_quiescence(&self) -> bool {
        self.quiescence.is_some()
    }

    /// Marks the call running and records its store.
    fn begin(&self, store_dropped: Option<CancellationToken>) -> bool {
        if let Some(quiescence) = &self.quiescence {
            let Some(store_dropped) = store_dropped else {
                debug_assert!(false, "a tracked call needs its store's drop token");
                return false;
            };
            let _ = quiescence.store_dropped.set(store_dropped);
        } else {
            debug_assert!(store_dropped.is_none());
        }
        self.state
            .update(|state| matches!(state, CallState::Pending).then_some(CallState::Running))
            .is_ok()
    }

    /// Marks the call finished without losing its stop reason.
    fn finish(&self) {
        let _ = self.state.update(|state| match state {
            CallState::Running => Some(CallState::Finished(None)),
            CallState::RunningStopped(stop) => Some(CallState::Finished(Some(stop))),
            CallState::Pending | CallState::Finished(_) | CallState::Stopped(_) => None,
        });
    }

    fn cancel(&self) {
        self.stop(Stop::Cancelled);
    }

    /// Stops a call that exceeded its deadline.
    fn expire(&self) {
        self.abandoned.arm();
        self.stop(Stop::Expired);
    }

    /// Records the first stop request.
    fn stop(&self, stop: Stop) {
        let stopped = self.state.update(|state| match state {
            CallState::Pending => Some(CallState::Stopped(stop)),
            CallState::Running => Some(CallState::RunningStopped(stop)),
            CallState::RunningStopped(_) | CallState::Finished(_) | CallState::Stopped(_) => None,
        });
        let Ok(before) = stopped else {
            return;
        };
        match before {
            // Wake the task holding the unstarted job.
            CallState::Pending => {
                self.settle();
                self.cancel.fire();
            }
            // Wake the instance and preserve the stop reason.
            CallState::Running => {
                self.abandoned.arm();
                self.cancel.fire();
            }
            CallState::RunningStopped(_) | CallState::Finished(_) | CallState::Stopped(_) => {
                debug_assert!(false, "only a pending or running call is stopped");
            }
        }
    }

    fn settle(&self) {
        if let Some(quiescence) = &self.quiescence {
            quiescence.settled.fire();
        }
    }

    fn stopped(&self) -> Option<Stop> {
        self.state.load().stopped()
    }

    async fn quiesced(&self) {
        let Some(quiescence) = &self.quiescence else {
            debug_assert!(false, "only a tracked call has a CancelHandle");
            return;
        };
        quiescence.settled.wait().await;
        if self.state.load().may_run()
            && let Some(store_dropped) = quiescence.store_dropped.get()
        {
            store_dropped.cancelled().await;
        }
    }

    fn is_quiesced(&self) -> bool {
        let Some(quiescence) = &self.quiescence else {
            debug_assert!(false, "only a tracked call has a CancelHandle");
            return false;
        };
        quiescence.settled.is_fired()
            && (!self.state.load().may_run()
                || quiescence
                    .store_dropped
                    .get()
                    .is_some_and(CancellationToken::is_cancelled))
    }
}

/// Marks a call settled when its job drops.
struct SettleOnDrop(Arc<CallControl>);

impl Drop for SettleOnDrop {
    fn drop(&mut self) {
        self.0.settle();
    }
}

/// A cloneable handle for canceling a dispatch and awaiting quiescence.
#[derive(Clone)]
pub struct CancelHandle(Arc<CallControl>);

impl CancelHandle {
    /// Requests cancellation. See [`DispatchHandle`] for placement behavior.
    pub fn cancel(&self) {
        self.0.cancel();
    }

    /// Waits until the host knows the call can no longer run.
    ///
    /// A warm or dedicated call may wait for its store to drop. A service call
    /// may wait for the guest to return or the service to stop. Guest work that
    /// outlives the call's answer is not tracked.
    pub async fn quiesced(&self) {
        self.0.quiesced().await;
    }

    /// Whether [`Self::quiesced`] would resolve now.
    pub fn is_quiesced(&self) -> bool {
        self.0.is_quiesced()
    }
}

impl std::fmt::Debug for CancelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelHandle")
            .field("quiesced", &self.is_quiesced())
            .finish_non_exhaustive()
    }
}

/// Cancels a call if dropped before its outcome is known.
struct CancelOnDrop {
    handle: CancelHandle,
    armed: bool,
}

impl CancelOnDrop {
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.handle.cancel();
        }
    }
}

/// A started dispatch with its outcome and cancellation controls.
///
/// Dropping the handle before [`Self::outcome`] completes cancels the call.
/// Wasmtime cannot stop one guest task without dropping its store
/// (bytecodealliance/wasmtime#11833), so placement determines the result:
///
///  * A pending call never runs, but may count against `maxInvocations`.
///  * A dedicated store is dropped.
///  * A warm instance is retired after its active calls drain.
///  * A service call runs until the guest returns.
///
/// [`Self::outcome`] returns [`DispatchCancelled`] immediately. Use
/// [`CancelHandle::quiesced`] to wait for guest execution to stop.
#[must_use = "dropping a DispatchHandle cancels its call"]
pub struct DispatchHandle {
    reply: oneshot::Receiver<anyhow::Result<()>>,
    deadline: Deadline,
    cancel_on_drop: CancelOnDrop,
}

/// The owner of a call's deadline.
enum Deadline {
    /// The task awaiting the outcome.
    Outcome(DispatchedCall),
    /// A watchdog task. `None` represents an unbounded instant.
    Watchdog(Option<tokio::time::Instant>),
}

/// When the outcome is awaited.
#[derive(Clone, Copy)]
enum Awaiting {
    /// Immediately.
    Now,
    /// Through a returned handle.
    Later,
}

impl DispatchHandle {
    /// Requests cancellation. See [the type docs](Self).
    pub fn cancel(&self) {
        self.cancel_on_drop.handle.cancel();
    }

    /// Returns a cloneable cancellation handle.
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel_on_drop.handle.clone()
    }

    /// Waits for the guest result, cancellation, or dispatch failure.
    ///
    /// The call uses [`GuestCall::deadline`]. Dropping this future cancels it.
    pub async fn outcome(self) -> anyhow::Result<()> {
        let DispatchHandle {
            reply,
            deadline,
            cancel_on_drop,
        } = self;
        let control = Arc::clone(&cancel_on_drop.handle.0);
        let answer = async {
            tokio::select! {
                biased;
                replied = reply => Some(replied),
                () = control.cancel.wait() => None,
            }
        };
        let answered = match deadline {
            Deadline::Outcome(dispatched) => dispatched.await_reply(answer).await,
            Deadline::Watchdog(Some(at)) => tokio::time::timeout_at(at, answer).await.ok(),
            Deadline::Watchdog(None) => Some(answer.await),
        };
        cancel_on_drop.disarm();
        let Some(answered) = answered else {
            control.expire();
            return Err(missed_deadline());
        };
        match control.stopped() {
            Some(Stop::Cancelled) => Err(DispatchCancelled.into()),
            Some(Stop::Expired) => Err(missed_deadline()),
            None => match answered {
                Some(Ok(outcome)) => outcome,
                // An unanswered job may have lost its instance.
                Some(Err(_)) | None => Err(anyhow::anyhow!(
                    "the instance serving the dispatched call went away"
                )),
            },
        }
    }
}

impl std::fmt::Debug for DispatchHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut shown = f.debug_struct("DispatchHandle");
        // Only `start` tracks quiescence.
        let cancel = &self.cancel_on_drop.handle;
        if cancel.0.tracks_quiescence() {
            shown.field("quiesced", &cancel.is_quiesced());
        }
        shown.finish_non_exhaustive()
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
    /// What this item's guest execution is recorded under. Resolved with the
    /// target rather than per call: the identity costs a read lock and cannot
    /// change under a resolved workload, and the attribute set is rebuilt only
    /// when a call names an operation this target has not carried before.
    metrics: Arc<DispatchMetrics>,
}

/// The attribute sets a target's dispatches are recorded under, one per
/// operation.
///
/// Keyed by [`GuestCall::describe`], which names a WIT export, so the map is
/// bounded by the interface set the item declares — never by traffic. Built
/// lazily because a target learns its operations only from the calls that
/// arrive on it, and cached because rebuilding a set costs the vector and five
/// strings on a delivery hot path to arrive at the same answer every time.
struct DispatchMetrics {
    identity: crate::observability::WorkloadIdentity,
    plugin: &'static str,
    by_operation: Mutex<std::collections::BTreeMap<Arc<str>, Arc<[opentelemetry::KeyValue]>>>,
}

impl DispatchMetrics {
    fn attributes(&self, operation: &str) -> Arc<[opentelemetry::KeyValue]> {
        let mut cached = self
            .by_operation
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(attributes) = cached.get(operation) {
            return Arc::clone(attributes);
        }
        let attributes = self.identity.attributes(self.plugin, operation);
        cached.insert(Arc::from(operation), Arc::clone(&attributes));
        attributes
    }
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
        plugin: &'static str,
    ) -> anyhow::Result<Self> {
        let item = if workload.is_service_item(item_id) {
            // Asked for by name, so a service that can never serve a dispatched
            // call is an error here rather than something to skip past.
            TargetItem::Service(Arc::new(workload.claim_service_dispatch()?))
        } else {
            let (id, pre, pool) = workload.component_dispatch(item_id).await?;
            TargetItem::Component { id, pre, pool }
        };
        let metrics = Arc::new(DispatchMetrics {
            identity: workload.component_identity(item_id).await,
            plugin,
            by_operation: Mutex::default(),
        });
        Ok(Self {
            // Copied once per target: cloning a `ResolvedWorkload` copies the
            // service's `Linker` by value, so it is worth paying here rather
            // than on every clone of the target.
            workload: Arc::new(workload.clone()),
            item,
            metrics,
        })
    }

    /// The workload this target belongs to.
    pub fn workload(&self) -> &ResolvedWorkload {
        &self.workload
    }

    /// Starts `call` and returns its handle without waiting.
    ///
    /// The deadline starts immediately and does not require awaiting the
    /// outcome. This method requires a Tokio runtime.
    pub fn start(&self, call: impl GuestCall) -> DispatchHandle {
        self.begin(Box::new(call), Awaiting::Later)
    }

    fn begin(&self, call: Box<dyn GuestCall>, awaiting: Awaiting) -> DispatchHandle {
        let attributes = self.metrics.attributes(call.describe());
        let (job, handle) = mint(call, attributes, awaiting);
        // A target outlives the workload it names: a plugin holds one until it
        // is unbound, and teardown begins before that. Without this a dispatch
        // racing a stop would build — and, for a pooled component, *park* — a
        // fresh instance in a workload the host has already stopped.
        if self.workload.released() {
            job.refuse(anyhow::anyhow!(
                "workload '{}' has stopped and takes no more dispatched calls",
                self.workload.id()
            ));
            return handle;
        }
        match &self.item {
            TargetItem::Component { id, pre, pool } => {
                start_on_component(&self.workload, id, pre, pool.as_ref(), job);
            }
            TargetItem::Service(claim) => start_on_service(claim.calls(), job),
        }
        handle
    }

    /// Runs `call` and waits for its outcome.
    ///
    /// The call uses [`GuestCall::deadline`]. Dropping this future cancels it.
    /// Use [`Self::start`] when the caller must await quiescence.
    pub async fn dispatch(&self, call: impl GuestCall) -> anyhow::Result<()> {
        self.begin(Box::new(call), Awaiting::Now).outcome().await
    }
}

/// Deliver a call to the workload's running service, and wait for it.
#[cfg(feature = "host-component-plugins")]
pub(crate) async fn dispatch_to_service(
    calls: &ServiceCalls,
    call: Box<dyn GuestCall>,
    attributes: Arc<[opentelemetry::KeyValue]>,
) -> anyhow::Result<()> {
    let (job, handle) = mint(call, attributes, Awaiting::Now);
    start_on_service(calls, job);
    handle.outcome().await
}

/// Queues a call on the workload's service.
fn start_on_service(calls: &ServiceCalls, job: GuestJob) {
    let Some(tx) = calls.sender() else {
        return job.refuse(anyhow::anyhow!(
            "the workload's service is not accepting dispatched calls"
        ));
    };
    match tx.try_send(job) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Closed(job)) => {
            job.refuse(anyhow::anyhow!(
                "the workload's service is no longer running"
            ));
        }
        // Wait for queue space without blocking cancellation.
        Err(mpsc::error::TrySendError::Full(job)) => {
            let control = Arc::clone(&job.control.0);
            tokio::spawn(async move {
                tokio::select! {
                    biased;
                    () = control.cancel.wait() => {}
                    sent = tx.send(job) => {
                        if let Err(mpsc::error::SendError(job)) = sent {
                            job.refuse(anyhow::anyhow!(
                                "the workload's service is no longer running"
                            ));
                        }
                    }
                }
            });
        }
    }
}

/// Starts a call on a warm or dedicated component instance.
fn start_on_component(
    workload: &Arc<ResolvedWorkload>,
    component_id: &Arc<str>,
    pre: &InstancePre<SharedCtx>,
    pool: Option<&Arc<InstancePool>>,
    job: GuestJob,
) {
    let (job, pool) = match pool {
        None => (job, None),
        Some(pool) => {
            let (job, room) = match pool.try_dispatch(InstanceJob::Guest(job)) {
                Dispatch::Sent => return,
                Dispatch::NeedsInstance(job) => (job, Some(Arc::clone(pool))),
                Dispatch::Saturated(job) => {
                    tracing::debug!(
                        component_id = %component_id,
                        "warm instances saturated; dispatching to a store of its own"
                    );
                    (job, None)
                }
            };
            let InstanceJob::Guest(job) = job else {
                debug_assert!(false, "a dispatched job cannot come back as another kind");
                return;
            };
            (job, room)
        }
    };
    tokio::spawn(hand_off(
        Arc::clone(workload),
        Arc::clone(component_id),
        pre.clone(),
        pool,
        job,
    ));
}

/// Builds an instance and assigns the call to the pool or a dedicated store.
async fn hand_off(
    workload: Arc<ResolvedWorkload>,
    component_id: Arc<str>,
    pre: InstancePre<SharedCtx>,
    pool: Option<Arc<InstancePool>>,
    job: GuestJob,
) {
    let mut job = job;
    // Reuse an instance declined by the pool as the dedicated store.
    let mut reclaimed = None;
    if let Some(pool) = pool {
        let built = match build_instance(&workload, &component_id, &pre, &job.control.0).await {
            Some(Ok(built)) => built,
            Some(Err(e)) => return job.refuse(e),
            None => return,
        };
        match pool.dispatch_on_new(built, InstanceJob::Guest(job)) {
            Ok(()) => return,
            Err(Declined {
                job: InstanceJob::Guest(declined),
                instance,
            }) => {
                job = declined;
                reclaimed = instance;
            }
            Err(_) => {
                debug_assert!(false, "a dispatched job cannot come back as another kind");
                return;
            }
        }
    }
    let ComponentInstance { store, instance } = match reclaimed {
        Some(built) => built,
        None => match build_instance(&workload, &component_id, &pre, &job.control.0).await {
            Some(Ok(built)) => built,
            Some(Err(e)) => return job.refuse(e),
            None => return,
        },
    };
    job.run_on_own_store(store, instance).await;
}

/// Builds an instance unless the call is canceled first.
async fn build_instance(
    workload: &ResolvedWorkload,
    component_id: &str,
    pre: &InstancePre<SharedCtx>,
    control: &CallControl,
) -> Option<anyhow::Result<ComponentInstance>> {
    let build = async {
        let mut store = workload.new_store(component_id).await?;
        let instance = pre.instantiate_async(&mut store).await?;
        anyhow::Ok(ComponentInstance { store, instance })
    };
    tokio::select! {
        biased;
        () = control.cancel.wait() => None,
        built = build => Some(built),
    }
}

/// Creates a job and its dispatch handle.
fn mint(
    call: Box<dyn GuestCall>,
    attributes: Arc<[opentelemetry::KeyValue]>,
    awaiting: Awaiting,
) -> (GuestJob, DispatchHandle) {
    let limit = call.deadline();
    let dispatched = DispatchedCall::new("plugin dispatch", limit);
    let (reply, reply_rx) = oneshot::channel();
    let job = GuestJob::with_quiescence(
        call,
        reply,
        dispatched.flag(),
        attributes,
        matches!(awaiting, Awaiting::Later),
    );
    let deadline = match awaiting {
        Awaiting::Now => Deadline::Outcome(dispatched),
        Awaiting::Later => {
            watch_deadline(dispatched, Arc::clone(&job.control.0));
            Deadline::Watchdog(tokio::time::Instant::now().checked_add(limit))
        }
    };
    let handle = DispatchHandle {
        reply: reply_rx,
        deadline,
        cancel_on_drop: CancelOnDrop {
            handle: job.cancel_handle(),
            armed: true,
        },
    };
    (job, handle)
}

/// Enforces a started call's deadline until it settles.
fn watch_deadline(dispatched: DispatchedCall, control: Arc<CallControl>) {
    tokio::spawn(async move {
        let Some(quiescence) = &control.quiescence else {
            debug_assert!(false, "a started call tracks settlement");
            return;
        };
        if dispatched
            .await_reply(quiescence.settled.wait())
            .await
            .is_none()
        {
            control.expire();
        }
    });
}

fn missed_deadline() -> anyhow::Error {
    anyhow::anyhow!("dispatched call produced no outcome within its deadline")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;

    use super::{
        Awaiting, CallControl, CancelHandle, DispatchCancelled, GuestCall, GuestCallFuture,
        GuestJob, ServiceCalls, SettleOnDrop, Stop, mint, watch_deadline,
    };
    use crate::engine::abandon::DispatchedCall;

    /// Builds tracked state for a test call.
    fn control() -> (Arc<CallControl>, DispatchedCall) {
        let dispatched = DispatchedCall::new("test", Duration::MAX);
        (
            Arc::new(CallControl::new(dispatched.flag(), true)),
            dispatched,
        )
    }

    /// A call that must not run.
    struct NeverRun;

    impl GuestCall for NeverRun {
        fn describe(&self) -> &str {
            "test"
        }
        fn call<'a>(
            self: Box<Self>,
            _: &'a wasmtime::component::Accessor<crate::engine::ctx::SharedCtx>,
            _: wasmtime::component::Instance,
        ) -> GuestCallFuture<'a> {
            unreachable!("these tests never serve a call")
        }
    }

    /// Cancellation before start prevents execution.
    #[test]
    fn a_cancel_before_the_start_means_the_call_never_runs() {
        let (control, _dispatched) = control();
        let handle = CancelHandle(Arc::clone(&control));

        handle.cancel();
        assert!(
            !control.begin(Some(CancellationToken::new())),
            "a call cancelled before it started must not start"
        );
        assert!(
            handle.is_quiesced(),
            "a call that never started has nothing left to wait for"
        );
    }

    /// A returned call does not wait for its store.
    #[test]
    fn a_call_that_returned_quiesces_without_waiting_on_its_store() {
        let (control, _dispatched) = control();
        let handle = CancelHandle(Arc::clone(&control));
        let store = CancellationToken::new();

        assert!(control.begin(Some(store.clone())));
        assert!(!handle.is_quiesced(), "a running call has not quiesced");
        control.finish();
        drop(SettleOnDrop(Arc::clone(&control)));
        assert!(
            handle.is_quiesced(),
            "a call that returned must not wait for its store to drop"
        );

        handle.cancel();
        assert!(
            !control.cancel.is_fired(),
            "cancelling a finished call must not reach whatever served it"
        );
    }

    /// A running call quiesces when its store drops.
    #[tokio::test]
    async fn a_call_given_up_on_quiesces_only_once_its_store_drops() {
        let (control, _dispatched) = control();
        let handle = CancelHandle(Arc::clone(&control));
        let store = CancellationToken::new();

        assert!(control.begin(Some(store.clone())));
        drop(SettleOnDrop(Arc::clone(&control)));
        assert!(!handle.is_quiesced());
        tokio::time::timeout(Duration::from_millis(50), handle.quiesced())
            .await
            .expect_err("the store the call was left running on has not dropped");

        store.cancel();
        assert!(handle.is_quiesced());
        tokio::time::timeout(Duration::from_millis(50), handle.quiesced())
            .await
            .expect("the store dropping is the last thing a given-up call waits for");
    }

    /// A canceled call remains active until its server releases it.
    #[test]
    fn cancelling_a_running_call_reaches_what_serves_it() {
        let (control, _dispatched) = control();
        let handle = CancelHandle(Arc::clone(&control));

        assert!(control.begin(Some(CancellationToken::new())));
        handle.cancel();
        assert!(control.cancel.is_fired());
        assert!(
            !handle.is_quiesced(),
            "a cancel is a request; the call has not been let go of yet"
        );
    }

    /// A deadline stops pending and running calls.
    #[test]
    fn a_deadline_stops_a_call_wherever_it_got_to() {
        let (pending, _dispatched) = control();
        pending.expire();
        assert!(!pending.begin(Some(CancellationToken::new())));
        assert_eq!(pending.stopped(), Some(Stop::Expired));

        let (running, _dispatched) = control();
        assert!(running.begin(Some(CancellationToken::new())));
        running.expire();
        assert!(
            running.cancel.is_fired(),
            "an expired call's instance is woken to give it up"
        );
        assert_eq!(
            running.stopped(),
            Some(Stop::Expired),
            "the reason is what has the instance record a timeout rather than a cancel"
        );
    }

    /// A job caught by its deadline reports the deadline, not a cancel.
    #[tokio::test]
    async fn an_expired_call_reads_as_a_deadline_not_a_cancel() {
        let (job, handle) = mint(Box::new(NeverRun), Arc::from([]), Awaiting::Later);
        job.control.0.expire();
        drop(job);
        let err = handle
            .outcome()
            .await
            .expect_err("an expired call must not report success");
        assert!(!err.is::<DispatchCancelled>(), "unexpected error: {err:#}");
        assert!(
            err.to_string().contains("within its deadline"),
            "unexpected error: {err:#}"
        );
    }

    /// A finished call keeps and reports its stop reason.
    #[tokio::test]
    async fn a_finished_call_keeps_its_stop_reason() {
        let (job, handle) = mint(Box::new(NeverRun), Arc::from([]), Awaiting::Later);
        let control = Arc::clone(&job.control.0);

        assert!(control.begin(Some(CancellationToken::new())));
        control.cancel();
        control.finish();
        let _ = job.reply.send(Ok(()));
        assert_eq!(control.stopped(), Some(Stop::Cancelled));
        assert!(
            !control.state.load().may_run(),
            "a call that answered cannot still be running"
        );
        let err = handle
            .outcome()
            .await
            .expect_err("a canceled call must not report success");
        assert!(err.is::<DispatchCancelled>(), "unexpected error: {err:#}");
    }

    /// A canceled, unanswered job reports cancellation.
    #[tokio::test]
    async fn a_cancelled_call_dropped_unanswered_reads_as_cancelled() {
        let (job, handle) = mint(Box::new(NeverRun), Arc::from([]), Awaiting::Later);
        handle.cancel();
        drop(job);
        let err = handle
            .outcome()
            .await
            .expect_err("a cancelled call must not report success");
        assert!(err.is::<DispatchCancelled>(), "unexpected error: {err:#}");
    }

    /// An unstopped, unanswered job reports a lost instance.
    #[tokio::test]
    async fn a_call_dropped_unanswered_and_unstopped_went_away() {
        let (job, handle) = mint(Box::new(NeverRun), Arc::from([]), Awaiting::Later);
        drop(job);
        let err = handle
            .outcome()
            .await
            .expect_err("an unanswered call must not report success");
        assert!(!err.is::<DispatchCancelled>(), "unexpected error: {err:#}");
        assert!(
            err.to_string().contains("went away"),
            "unexpected error: {err:#}"
        );
    }

    /// A watchdog expires an unawaited call.
    #[tokio::test(start_paused = true)]
    async fn a_watched_deadline_expires_a_call_nobody_awaits() {
        let dispatched = DispatchedCall::new("test", Duration::from_secs(1));
        let control = Arc::new(CallControl::new(dispatched.flag(), true));
        watch_deadline(dispatched, Arc::clone(&control));

        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(control.stopped(), Some(Stop::Expired));
        assert!(
            !control.begin(Some(CancellationToken::new())),
            "a call past its deadline must not start"
        );
    }

    /// A watchdog leaves a settled call unstopped.
    #[tokio::test(start_paused = true)]
    async fn a_watched_deadline_lets_go_of_a_call_that_settled() {
        let dispatched = DispatchedCall::new("test", Duration::from_secs(1));
        let control = Arc::new(CallControl::new(dispatched.flag(), true));
        watch_deadline(dispatched, Arc::clone(&control));

        assert!(control.begin(Some(CancellationToken::new())));
        control.finish();
        drop(SettleOnDrop(Arc::clone(&control)));
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(
            control.stopped(),
            None,
            "a call that settled in time was not stopped"
        );
    }

    /// Dropping an unserved job settles it.
    #[test]
    fn a_job_dropped_unserved_quiesces() {
        let dispatched = DispatchedCall::new("test", Duration::MAX);
        let job = GuestJob::with_quiescence(
            Box::new(NeverRun),
            tokio::sync::oneshot::channel().0,
            dispatched.flag(),
            Arc::from([]),
            true,
        );
        let handle = job.cancel_handle();

        assert!(!handle.is_quiesced());
        drop(job);
        assert!(handle.is_quiesced());
    }

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
