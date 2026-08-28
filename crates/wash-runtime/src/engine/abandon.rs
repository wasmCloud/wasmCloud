//! Teardown for guest work whose caller has given up on it — even when the
//! guest never yields.
//!
//! Every other teardown the host owns (`tokio::time::timeout`, task aborts,
//! pool retirement) is a future on the very task a non-yielding guest is
//! blocking, so none of them can fire against the one failure mode that
//! matters most: a guest spinning in compiled code. The pieces here close that
//! hole, split across the only two places that can act:
//!
//! - **Outside the store**, a dispatcher hands its call a [`DispatchedCall`]:
//!   the deadline, and a flag ([`AbandonFlag`]) it arms once the result is no
//!   longer wanted — the deadline passed, or whoever asked went away.
//! - **Inside the store**, the task serving the call registers that flag with
//!   the store's [`AbandonedCalls`] for exactly as long as the call runs, and
//!   [`arm_epoch_deadline`] wires wasmtime's epoch callback — compiled into
//!   the guest's own loop back-edges, the only host code a non-yielding guest
//!   cannot block — to read it.
//!
//! A store is acted on only when three things hold: a call on it has been
//! abandoned longer than [`crate::timeouts::abandoned_call_grace`] and is
//! still registered, *every* other registered call has been abandoned too,
//! and the guest has since run through a grace's worth of execution.
//! Execution is credited from the callback's own fires (`ExecutionCredit`):
//! each fire proves the guest was running and is worth at most one sampling
//! window, and only a pause too long to be scheduler delay clears the credit.
//! Abandonment alone would not do: dropping the dispatcher arms the flag, so
//! an ordinary client disconnect would condemn a store whose guest is healthy
//! and merely slow.
//!
//! Registration, not the sampling above, is what keeps a healthy guest out of
//! this. Every ingress onto a store that outlives its call (a service
//! singleton, a pooled instance, a plugin) runs it through
//! [`watch_until_abandoned`], which deregisters the call once it has been
//! abandoned for the grace and the guest has yielded. A new ingress onto such a
//! store must go through it, or a merely slow guest there will be trapped.
//!
//! A store built to serve one call and be discarded (a cold one-shot HTTP
//! store, an ephemeral linked call, a per-message store) does not need it:
//! trapping it ends only the call that was already abandoned.
//!
//! The all-abandoned condition is what keeps the per-store blast radius safe:
//! inside one sampling window a burst-then-await guest (frames, tokens, short
//! outbound calls) is indistinguishable from a pinned one, so the trap waits
//! until it would end nothing still wanted. Teardown of a really pinned store
//! is delayed by that, not lost — such a guest admits no new calls, and the
//! calls already on it are abandoned by their own deadlines in turn.
//!
//! No judgement of *speed* is made anywhere here. The epoch advances on wall
//! clock, so it can report whether guest code is running, never whether it is
//! getting anywhere: a store pegged at 100% CPU serving calls its callers
//! still want is left alone.
//!
//! What this cannot see: a guest burning CPU with *no* call outstanding
//! against it — one that delivered its response and then kept spinning.
//! Abandonment is keyed on calls, and there is no call left to abandon.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::engine::ctx::SharedCtx;

/// Milliseconds since the first flag was minted, biased by +1 so that `0` can
/// mean "not armed" inside an [`AbandonFlag`].
fn now_millis() -> u64 {
    static ANCHOR: LazyLock<Instant> = LazyLock::new(Instant::now);
    u64::try_from(ANCHOR.elapsed().as_millis()).unwrap_or(u64::MAX - 1) + 1
}

/// When a call's dispatcher stopped wanting its result, or nothing if it still
/// does. Minted only by [`DispatchedCall`], which is what makes a job type's
/// `abandoned` field proof that its dispatcher actually enforces a deadline.
pub struct AbandonFlag {
    armed_at: AtomicU64,
    /// Wakes [`Self::abandoned_for`] when arming happens.
    armed: tokio::sync::Notify,
}

impl AbandonFlag {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            armed_at: AtomicU64::new(0),
            armed: tokio::sync::Notify::new(),
        })
    }

    /// Record the abandonment instant; only the first arming counts.
    pub(crate) fn arm(&self) {
        if self
            .armed_at
            .compare_exchange(0, now_millis(), Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            self.armed.notify_waiters();
        }
    }

    /// Resolves once this call has been abandoned for `grace`.
    ///
    /// Awaited from inside the store the call runs in, where it shares a task
    /// with the guest call and so resolves only if the guest yields. See
    /// [`watch_until_abandoned`].
    async fn abandoned_for(&self, grace: Duration) {
        loop {
            // Registered before the check. `notify_waiters` stores no permit,
            // so an arming in between would otherwise be missed.
            let armed = self.armed.notified();
            let Some(at) = self.armed_at() else {
                armed.await;
                continue;
            };
            let elapsed = Duration::from_millis(now_millis().saturating_sub(at));
            if let Some(left) = grace.checked_sub(elapsed) {
                tokio::time::sleep(left).await;
            }
            return;
        }
    }

    /// Arm this flag when `deadline` elapses. For re-bounding a call that
    /// outlives its reply (a post-reply stream drain), and for waiters that
    /// themselves live inside the dispatched-to store.
    ///
    /// The timer sleeps the whole deadline, so the handle must be held by the
    /// work it bounds and dropped when that work ends; detaching one per call
    /// accumulates a task and a timer entry per call on healthy traffic.
    #[must_use = "dropping the handle immediately cancels the timer; bind it to the work it bounds"]
    pub(crate) fn arm_after(self: Arc<Self>, deadline: Duration) -> ArmTimer {
        ArmTimer(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
            async move {
                tokio::time::sleep(deadline).await;
                self.arm();
            },
        )))
    }

    fn armed_at(&self) -> Option<u64> {
        match self.armed_at.load(Ordering::Relaxed) {
            0 => None,
            at => Some(at),
        }
    }
}

/// Cancels a pending [`AbandonFlag::arm_after`] timer when dropped.
pub(crate) struct ArmTimer(#[expect(dead_code)] tokio_util::task::AbortOnDropHandle<()>);

/// Run `call`, keeping it registered on its store's [`AbandonedCalls`] only
/// while the epoch callback could usefully act on it.
///
/// The call is deregistered once it has been abandoned for `grace` and the
/// guest has yielded. The grace timer shares a task with the guest call, so a
/// guest awaiting anything lets it run and a guest pinned in compiled code
/// cannot: reaching the timer is itself proof the guest is not wedged.
///
/// The call keeps running either way, and its result is still delivered. A
/// `wasmcloud:host/workload-lifecycle` bind is uncancellable, and the host
/// defers a rollback unbind until it completes so that whatever it provisioned
/// late is still reclaimed.
///
/// A guest that yields once after being abandoned and only then starts spinning
/// is no longer visible here, the same blind spot as a guest that spins after
/// delivering its response.
pub(crate) async fn watch_until_abandoned<F: Future>(
    calls: &Arc<AbandonedCalls>,
    flag: Arc<AbandonFlag>,
    call: F,
) -> F::Output {
    let grace = crate::timeouts::abandoned_call_grace();
    let mut registration = Some(calls.watch(Arc::clone(&flag)));
    let mut call = std::pin::pin!(call);
    let deregister = flag.abandoned_for(grace);
    let mut deregister = std::pin::pin!(deregister);
    loop {
        tokio::select! {
            output = &mut call => return output,
            // Disabled once it has fired; a completed future must not be
            // polled again.
            () = &mut deregister, if registration.is_some() => {
                registration = None;
            }
        }
    }
}

/// The in-flight calls on one store, so the store's epoch callback can see
/// which of them have been abandoned. One per store, on [`SharedCtx`].
#[derive(Default)]
pub struct AbandonedCalls(Mutex<Vec<Arc<AbandonFlag>>>);

impl AbandonedCalls {
    /// Watch `flag` until the returned guard drops — held by the task serving
    /// the call, for exactly as long as the call runs.
    pub fn watch(self: &Arc<Self>, flag: Arc<AbandonFlag>) -> AbandonedGuard {
        self.lock().push(Arc::clone(&flag));
        AbandonedGuard {
            calls: Arc::clone(self),
            flag,
        }
    }

    /// What the epoch callback needs to know about this store's calls.
    ///
    /// Answers every question under one lock, so the callback always acts on a
    /// state that held at a single instant. Reading them separately lets a call
    /// deregister in between, which can leave an empty list looking like
    /// "everything is abandoned" and trap a store with nothing outstanding.
    fn survey(&self, grace: Duration) -> CallSurvey {
        let list = self.lock();
        let grace = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX);
        // `now` is only worth computing once a flag is actually armed; the
        // common case (none are) stays free of clock reads.
        let mut now = None;
        let mut survey = CallSurvey {
            watched: list.len(),
            armed: 0,
            any_past_grace: false,
        };
        for flag in list.iter() {
            let Some(at) = flag.armed_at() else { continue };
            survey.armed += 1;
            let now = *now.get_or_insert_with(now_millis);
            survey.any_past_grace |= now.saturating_sub(at) >= grace;
        }
        survey
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Arc<AbandonFlag>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One consistent look at a store's registered calls.
#[derive(Clone, Copy)]
struct CallSurvey {
    watched: usize,
    armed: usize,
    /// Whether any armed call has been abandoned for longer than the grace.
    any_past_grace: bool,
}

impl CallSurvey {
    /// There is at least one registered call and every one of them is
    /// abandoned, so trapping the store would end nothing still wanted.
    fn all_abandoned(self) -> bool {
        self.watched > 0 && self.armed == self.watched
    }
}

/// Guest execution credited against one store, accumulated from the epoch
/// callback's own fires while the host wants the store back.
///
/// The callback runs only while guest code does, so each fire proves the
/// guest was executing at that instant — and is worth at most one sampling
/// window ([`EXECUTION_WINDOW_MS`]) of credit, however wide the gap before it.
/// The per-fire cap keeps a long await from donating its whole off-CPU length,
/// and crediting the gap rather than resetting on it keeps a pinned guest whose
/// fires land late under scheduler load accumulating instead of having its
/// record wiped. Only a gap no scheduler could plausibly cause
/// ([`reset_threshold_millis`]) proves a real pause and clears the credit.
///
/// This is not a measure of CPU. A gap under one window is credited whole, and
/// the gap between fires never falls below one window however idle the store
/// is, so a guest awaiting in sub-window hops banks credit as fast as a pinned
/// one. [`watch_until_abandoned`] is what separates the two; this is only a
/// backstop for a call still registered after it.
///
/// Measured here rather than from the host side because the callback forces a
/// yield on every fire, which would refresh any host-side liveness signal and
/// make a pinned guest look healthy.
#[derive(Default)]
struct ExecutionCredit {
    credited_millis: u64,
    /// When this callback last fired.
    last_fire: Option<u64>,
}

/// What one fire of the epoch callback proved, seen by the two things that
/// read it.
struct Fire {
    /// Execution this fire alone proves, capped at one sampling window: what
    /// [`GuestExecution`] adds to its running total.
    executed_millis: u64,
    /// Execution credited since the store was last not wanted back: what the
    /// abandonment check tests against its grace.
    credited_millis: u64,
}

impl ExecutionCredit {
    /// Record a fire, in milliseconds.
    fn observe(&mut self, now: u64) -> Fire {
        let gap = self.last_fire.map_or(0, |last| now.saturating_sub(last));
        // A gap only a real pause explains proves nothing about execution, so
        // it neither meters nor keeps what came before it.
        let executed_millis = if gap > reset_threshold_millis() {
            self.credited_millis = 0;
            0
        } else {
            let executed = gap.min(EXECUTION_WINDOW_MS);
            self.credited_millis = self.credited_millis.saturating_add(executed);
            executed
        };
        self.last_fire = Some(now);
        Fire {
            executed_millis,
            credited_millis: self.credited_millis,
        }
    }

    /// Clear the credit: the store is not wanted back right now, so whatever
    /// its guest is doing is not held against it.
    fn reset(&mut self) {
        self.credited_millis = 0;
    }
}

/// Guest execution on one store since it was built, in milliseconds, sampled
/// by the store's epoch callback.
///
/// Read as a delta across a call, this is what
/// [`crate::observability::ExecutionTimeMeter`] reports — it replaced fuel,
/// whose per-block counters are compiled into the guest and cost throughput
/// even when nothing is metered. This is free: the callback the samples come
/// from is armed on every store anyway, for the teardown this module exists
/// for.
///
/// What it costs instead is resolution, in both directions.
/// The callback fires at the first back-edge past a deadline of
/// [`EPOCH_DEADLINE_TICKS`] ticks, and [`rearm_for_call`] restarts that
/// countdown per call, so **a call whose guest runs for less than one sampling
/// window ([`EXECUTION_WINDOW_MS`]) never fires it and meters as zero**. Read
/// the histogram as "calls that burned at least a window of guest execution",
/// never as a total: a host serving nothing but short calls reports zero, and
/// is not idle.
///
/// The count is per *store*, not per call, so a delta taken across one call on
/// a store serving several at once — a pooled instance, a plugin store, any
/// component under the concurrent ABI — includes its neighbours' execution.
/// Fuel was per-store in exactly the same way.
#[derive(Default, Debug)]
pub struct GuestExecution(AtomicU64);

impl GuestExecution {
    /// Guest execution credited to this store so far, in milliseconds.
    pub fn millis(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    fn add(&self, millis: u64) {
        if millis > 0 {
            self.0.fetch_add(millis, Ordering::Relaxed);
        }
    }
}

/// The most execution one fire-to-fire gap can prove: fires land one sampling
/// window apart while a guest runs straight through.
const EXECUTION_WINDOW_MS: u64 =
    crate::engine::EPOCH_TICK.as_millis() as u64 * EPOCH_DEADLINE_TICKS;

/// The finest execution a call can be metered at: below one window the epoch
/// callback never fires, so the call records zero. See [`GuestExecution`].
pub fn sampling_window() -> Duration {
    Duration::from_millis(EXECUTION_WINDOW_MS)
}

/// A gap between fires that only a real pause can explain — the guest waiting
/// on host I/O measured in seconds, or going idle — never scheduler delay or
/// a brief await. Not derived from the window: it models what schedulers do,
/// not how often we sample, which is why it is tunable on its own
/// ([`crate::timeouts::abandoned_call_pause_threshold`]).
fn reset_threshold_millis() -> u64 {
    u64::try_from(crate::timeouts::abandoned_call_pause_threshold().as_millis()).unwrap_or(u64::MAX)
}

/// Stops watching a call's flag when dropped, so a completed call leaves
/// nothing behind — and so an abandoned flag whose call has ended is invisible
/// to the epoch callback, which is what makes arming safe on every path.
pub struct AbandonedGuard {
    calls: Arc<AbandonedCalls>,
    flag: Arc<AbandonFlag>,
}

impl Drop for AbandonedGuard {
    fn drop(&mut self) {
        self.calls.lock().retain(|f| !Arc::ptr_eq(f, &self.flag));
    }
}

/// Arms a dispatched call's flag when dropped, unless the reply arrived first.
///
/// Drop-armed rather than only timeout-armed so that a dispatcher future
/// cancelled outright — a client disconnect drops it without running any of
/// its code — counts the same as its deadline passing.
pub(crate) struct AbandonOnDrop {
    flag: Arc<AbandonFlag>,
    arm_on_drop: bool,
}

impl AbandonOnDrop {
    fn new(flag: Arc<AbandonFlag>) -> Self {
        Self {
            flag,
            arm_on_drop: true,
        }
    }

    /// The reply arrived and is wanted; leave the call alone.
    pub(crate) fn disarm(mut self) {
        self.arm_on_drop = false;
    }

    fn flag(&self) -> Arc<AbandonFlag> {
        Arc::clone(&self.flag)
    }
}

impl Drop for AbandonOnDrop {
    fn drop(&mut self) {
        if self.arm_on_drop {
            self.flag.arm();
        }
    }
}

/// One call handed to a store the dispatcher does not drive: the deadline the
/// dispatcher will enforce, and the flag the store-side task must register.
///
/// This is the only source of [`AbandonFlag`]s, and every job type carries one
/// as a required field — so a new ingress cannot be wired up without deciding
/// its deadline, and cannot compile with the waiting accidentally placed
/// inside the store it is waiting on.
///
/// Owns an [`AbandonOnDrop`] rather than a bare flag so that dropping it on
/// *any* path — including an `await_reply` future cancelled before its first
/// poll, after the job was already sent — abandons the call rather than
/// leaving the store to serve it forever.
pub(crate) struct DispatchedCall {
    watch: AbandonOnDrop,
    deadline: Duration,
    /// Names the ingress in the timeout log line.
    what: &'static str,
}

impl DispatchedCall {
    pub(crate) fn new(what: &'static str, deadline: Duration) -> Self {
        Self {
            watch: AbandonOnDrop::new(AbandonFlag::new()),
            deadline,
            what,
        }
    }

    /// The flag to carry in the job; the store-side task registers it with
    /// [`AbandonedCalls::watch`] for the life of the call.
    pub(crate) fn flag(&self) -> Arc<AbandonFlag> {
        self.watch.flag()
    }

    /// Await the call's reply, abandoning the call if the deadline passes or
    /// this future is dropped. `None` means no reply is coming.
    pub(crate) async fn await_reply<F: Future>(self, reply: F) -> Option<F::Output> {
        let (output, watch) = self.await_head(reply).await?;
        watch.disarm();
        Some(output)
    }

    /// For a dispatcher that must await from *inside* the store it dispatched
    /// to (a lifecycle replay, whose serve loop shares the plugin store): its
    /// own timeout can be starved by the very guest it bounds, so consume the
    /// call and arm the flag from a timer instead.
    ///
    /// The returned [`ArmTimer`] must be held for as long as the call it
    /// bounds — dropping it cancels the arming, and dropping it once the call
    /// has finished is exactly the point.
    #[cfg_attr(not(feature = "host-component-plugins"), allow(dead_code))]
    #[must_use = "hold the timer for the life of the call; dropping it cancels the arming"]
    pub(crate) fn arm_on_timer(self) -> ArmTimer {
        let Self {
            watch, deadline, ..
        } = self;
        let timer = watch.flag().arm_after(deadline);
        watch.disarm();
        timer
    }

    /// Like [`await_reply`], but the reply is only the *head* of the exchange:
    /// on delivery the returned [`AbandonOnDrop`] keeps the call condemned-on-
    /// drop, for a dispatcher that goes on streaming (an HTTP response body)
    /// after the reply arrives.
    ///
    /// [`await_reply`]: Self::await_reply
    pub(crate) async fn await_head<F: Future>(
        self,
        reply: F,
    ) -> Option<(F::Output, AbandonOnDrop)> {
        let Self {
            watch,
            deadline,
            what,
        } = self;
        match tokio::time::timeout(deadline, reply).await {
            Ok(output) => Some((output, watch)),
            Err(_) => {
                tracing::error!(
                    ingress = what,
                    timeout = ?deadline,
                    "dispatched call produced no reply within its deadline; abandoning it so its \
                     store can end it"
                );
                // `watch` drops here, arming the flag.
                None
            }
        }
    }
}

/// How many epoch ticks of *continuous guest execution* between deadline
/// checks — how promptly an abandoned call is noticed once its grace runs out,
/// and how often a busy guest yields its worker thread back to the executor.
///
/// Neither purpose wants this fine. Noticing within ~100ms is ample against
/// deadlines measured in seconds, and yielding every ~100ms of unbroken guest
/// execution is enough to keep a pinned guest from starving the executor (and
/// tokio's time driver) for longer than that. What rules out a *finer* setting
/// is the yield itself, which requeues the store's task through the executor.
///
/// The epoch advances on wall clock whether or not a guest is running, so this
/// counts continuous execution only because [`rearm_for_call`] restarts it per
/// call; left armed at construction, an idle store is already past its deadline
/// when its next call arrives.
const EPOCH_DEADLINE_TICKS: u64 = 10;

/// Re-arm the epoch deadline at the start of a call, so its countdown measures
/// that call's own execution. See [`EPOCH_DEADLINE_TICKS`].
pub(crate) fn rearm_for_call(store: &mut impl wasmtime::AsContextMut<Data = SharedCtx>) {
    store
        .as_context_mut()
        .set_epoch_deadline(EPOCH_DEADLINE_TICKS);
}

/// What a store does about a call that is both abandoned and wedged; see
/// [`arm_epoch_deadline`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AbandonedCallPolicy {
    /// Trap the store. For a store that can be replaced: an ephemeral store
    /// serves one call, a pooled instance is reaped and rebuilt on the next
    /// call, and a trapped service is restarted by its supervisor. Everything
    /// sharing the store goes down with it, and by this point was already
    /// stuck behind the pinned guest anyway.
    Trap,
    /// Warn first, trap only once the same conditions have held for
    /// [`crate::timeouts::abandoned_call_escalation`]. For a host component
    /// plugin's store, which serves every tenant at once, so the restart it
    /// costs is worth delaying while there is any chance the guest frees the
    /// store on its own. Per-task cancellation
    /// (bytecodealliance/wasmtime#11833) is what would end the one call
    /// instead.
    WarnThenTrap,
}

/// Let the host end guest work that has been abandoned *and* has wedged its
/// store, even though the guest never yields.
///
/// Every store needs this call whether or not anything will ever abandon a
/// call on it — a store that never sets a deadline traps the moment it runs
/// any guest code, and the host cannot install one later.
///
/// The callback is the enforcement point because it is the only one left: a
/// guest that never yields never returns from its poll, so every host future
/// on that store — timeouts included — is stuck behind it. wasmtime compiles
/// the deadline check into the guest's own loop back-edges, which is the one
/// place a pinned guest cannot avoid. All three conditions are required — an
/// armed flag alone only says nobody wants the result, which a client
/// disconnect is enough to cause. The module docs ([`crate::engine::abandon`])
/// cover it.
pub(crate) fn arm_epoch_deadline(
    store: &mut wasmtime::Store<SharedCtx>,
    policy: AbandonedCallPolicy,
) {
    let abandoned = Arc::clone(&store.data().abandoned);
    let executed = Arc::clone(&store.data().executed);
    let store_id = Arc::clone(&store.data().active_ctx.store_id);
    let grace = crate::timeouts::abandoned_call_grace();
    let escalation = crate::timeouts::abandoned_call_escalation();
    let grace_millis = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX);
    let escalation_millis = u64::try_from(escalation.as_millis()).unwrap_or(u64::MAX);
    let mut credit = ExecutionCredit::default();
    // Reset below whenever the warned-about condition stops holding, so every
    // episode is logged.
    let mut warned = false;

    store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
    store.epoch_deadline_callback(move |_| {
        // Every fire counts, or the credit means nothing.
        let fire = credit.observe(now_millis());
        // Metering reads the running total, so it accrues on every fire —
        // including the ones `keep_running` clears the abandonment credit on.
        executed.add(fire.executed_millis);
        let executed_for = fire.credited_millis;

        // Yield, never merely continue: this callback is the only point at
        // which a pinned guest hands the host back its thread. Ridden straight
        // through, the guest keeps the worker — and if that worker holds
        // tokio's time driver, *no timer on the runtime fires*, including the
        // very deadline whose expiry would abandon this call.
        let keep_running = |warned: &mut bool, credit: &mut ExecutionCredit| {
            // Every path through here means the warned-about condition no
            // longer holds, so the next episode warns again. Waiting for zero
            // armed calls instead would rarely fire on a busy plugin store,
            // where every dispatcher drop arms one.
            *warned = false;
            // Execution only counts against a store the host wants back:
            // otherwise a service's own background wakes (a run loop ticking
            // faster than the reset threshold) would bank credit for the
            // store's whole lifetime and any later abandonment would trap at
            // the grace with no execution since.
            credit.reset();
            Ok(wasmtime::UpdateDeadline::Yield(EPOCH_DEADLINE_TICKS))
        };

        // The conditions below must all hold at the same instant to justify
        // trapping, so read them once.
        let survey = abandoned.survey(grace);
        if !survey.any_past_grace {
            return keep_running(&mut warned, &mut credit);
        }
        // A call is still wanted: trapping the store would take it too — and
        // this is what keeps a yielding guest's store alive however its wakes
        // land relative to the sampling window.
        if !survey.all_abandoned() {
            return keep_running(&mut warned, &mut credit);
        }
        // Wanted back, but the guest has not yet run through a grace's worth
        // of execution since then: it is waiting, or finishing.
        if executed_for < grace_millis {
            return Ok(wasmtime::UpdateDeadline::Yield(EPOCH_DEADLINE_TICKS));
        }
        if policy == AbandonedCallPolicy::WarnThenTrap && executed_for < escalation_millis {
            if !warned {
                warned = true;
                tracing::warn!(
                    store_id = %store_id,
                    escalation = ?escalation,
                    "an abandoned call has wedged this store; it is shared, so it is not trapped \
                     before the escalation"
                );
            }
            return Ok(wasmtime::UpdateDeadline::Yield(EPOCH_DEADLINE_TICKS));
        }
        tracing::error!(
            store_id = %store_id,
            executed_ms = executed_for,
            "an abandoned call has wedged this store past its grace; trapping it"
        );
        Ok(wasmtime::UpdateDeadline::Interrupt)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flag armed and then deregistered must be invisible: this is what makes
    /// arming safe on every completion path (disconnects included) rather than
    /// only on timeouts.
    #[test]
    fn deregistered_flags_are_invisible() {
        let calls = Arc::new(AbandonedCalls::default());
        let call = DispatchedCall::new("test", Duration::ZERO);
        let flag = call.flag();
        // Keep `call` alive: dropping it would arm the flag itself.
        let guard = calls.watch(Arc::clone(&flag));

        assert!(!calls.survey(Duration::ZERO).any_past_grace);
        flag.arm();
        assert!(calls.survey(Duration::ZERO).any_past_grace);
        drop(guard);
        assert!(!calls.survey(Duration::ZERO).any_past_grace);
    }

    #[test]
    fn a_wanted_call_blocks_all_abandoned() {
        let calls = Arc::new(AbandonedCalls::default());
        let given_up = DispatchedCall::new("test", Duration::ZERO);
        let wanted = DispatchedCall::new("test", Duration::ZERO);
        let _given_up_guard = calls.watch(given_up.flag());
        let wanted_guard = calls.watch(wanted.flag());

        given_up.flag().arm();
        assert!(calls.survey(Duration::ZERO).any_past_grace);
        assert!(!calls.survey(Duration::ZERO).all_abandoned());
        // The wanted call ends (or is abandoned in turn); nothing on the
        // store is wanted any more.
        drop(wanted_guard);
        assert!(calls.survey(Duration::ZERO).all_abandoned());
    }

    /// The grace holds the callback off a freshly abandoned call, and only the
    /// first arming sets the clock.
    #[test]
    fn grace_is_measured_from_first_arming() {
        let calls = Arc::new(AbandonedCalls::default());
        let call = DispatchedCall::new("test", Duration::ZERO);
        let flag = call.flag();
        let _guard = calls.watch(Arc::clone(&flag));

        flag.arm();
        assert!(!calls.survey(Duration::from_secs(3600)).any_past_grace);
        // Re-arming must not push the abandonment instant forward.
        flag.arm();
        assert!(calls.survey(Duration::ZERO).any_past_grace);
    }

    /// One fire proves at most one window of execution however wide the gap
    /// before it, sub-reset gaps (scheduler delay included) never wipe what a
    /// pinned guest accrued, and only a real pause clears the credit.
    #[test]
    fn execution_credit_caps_each_gap_and_resets_only_on_a_real_pause() {
        const W: u64 = EXECUTION_WINDOW_MS;
        let mut c = ExecutionCredit::default();
        // First fire: nothing before it to credit.
        assert_eq!(c.observe(1_000).credited_millis, 0);
        // Straight-through execution: every window fully credited.
        assert_eq!(c.observe(1_000 + W).credited_millis, W);
        // A wide-but-sub-reset gap — a brief await, or a slow requeue under
        // load — donates one window, and does not wipe the accrual.
        assert_eq!(c.observe(1_000 + W + 900).credited_millis, 2 * W);
        assert_eq!(c.observe(1_000 + W + 900 + W).credited_millis, 3 * W);
        // A gap past the reset threshold is a real pause: credit cleared.
        let after_pause = 1_000 + W + 900 + W + reset_threshold_millis() + 1;
        assert_eq!(c.observe(after_pause).credited_millis, 0);
        assert_eq!(c.observe(after_pause + W).credited_millis, W);
        // And the callback clears it whenever the store is not wanted back.
        c.reset();
        assert_eq!(c.observe(after_pause + 2 * W).credited_millis, W);
    }

    /// The metered total accrues per fire and survives the resets the
    /// abandonment credit takes — a store the host is not waiting on is still
    /// running its guest, and that execution is what the meter reports.
    #[test]
    fn metered_execution_accrues_across_every_reset() {
        const W: u64 = EXECUTION_WINDOW_MS;
        let mut c = ExecutionCredit::default();
        let total = GuestExecution::default();

        for now in [1_000, 1_000 + W, 1_000 + W + 900] {
            total.add(c.observe(now).executed_millis);
            // Whatever the abandonment credit does, the meter keeps counting.
            c.reset();
        }
        // The first fire proves nothing; the two after it a window each.
        assert_eq!(total.millis(), 2 * W);

        // A pause is not execution, so it adds nothing at all.
        let after_pause = 1_000 + W + 900 + reset_threshold_millis() + 1;
        total.add(c.observe(after_pause).executed_millis);
        assert_eq!(total.millis(), 2 * W);
    }

    /// A call is deregistered a grace after being abandoned, but only because
    /// the future measuring that grace got to run, which is the whole test.
    #[tokio::test(start_paused = true)]
    async fn a_yielding_call_is_deregistered_a_grace_after_abandonment() {
        let calls = Arc::new(AbandonedCalls::default());
        let call = DispatchedCall::new("test", Duration::from_secs(1));
        let flag = call.flag();
        let grace = crate::timeouts::abandoned_call_grace();

        // A guest that yields: it awaits, so the deregistration timer runs.
        let yielding = watch_until_abandoned(&calls, Arc::clone(&flag), async {
            tokio::time::sleep(grace * 10).await;
            7u32
        });
        let mut yielding = std::pin::pin!(yielding);

        // Registered while the call is wanted, however long it runs.
        tokio::time::timeout(grace * 2, &mut yielding)
            .await
            .unwrap_err();
        assert_eq!(calls.survey(Duration::ZERO).watched, 1);

        // Abandoned: still registered until the grace is out...
        drop(call);
        assert!(flag.armed_at().is_some());
        tokio::time::timeout(grace / 2, &mut yielding)
            .await
            .unwrap_err();
        assert_eq!(
            calls.survey(Duration::ZERO).watched,
            1,
            "deregistered before the grace was out"
        );

        // ...and gone once it is, with the call still running and still
        // delivering its result afterwards.
        tokio::time::timeout(grace, &mut yielding)
            .await
            .unwrap_err();
        assert_eq!(
            calls.survey(Duration::ZERO).watched,
            0,
            "a yielding call must stop being visible to the epoch callback"
        );
        assert_eq!(yielding.await, 7, "the call must still run to completion");
    }

    /// `await_reply` disarms on delivery and arms on deadline or drop.
    #[tokio::test(start_paused = true)]
    async fn await_reply_arms_on_deadline_and_drop_only() {
        // Delivered: no arming.
        let call = DispatchedCall::new("test", Duration::from_secs(1));
        let flag = call.flag();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tx.send(7u32).unwrap();
        assert_eq!(call.await_reply(rx).await, Some(Ok(7)));
        assert!(flag.armed_at().is_none());

        // Deadline passed: armed.
        let call = DispatchedCall::new("test", Duration::from_secs(1));
        let flag = call.flag();
        let (_tx, rx) = tokio::sync::oneshot::channel::<u32>();
        assert_eq!(call.await_reply(rx).await, None);
        assert!(flag.armed_at().is_some());

        // Future dropped without ever being polled (a disconnect can cancel
        // the dispatcher at its previous await, after the job was sent): the
        // call travels inside the future, so this must still arm.
        let call = DispatchedCall::new("test", Duration::from_secs(1));
        let flag = call.flag();
        let (_tx, rx) = tokio::sync::oneshot::channel::<u32>();
        drop(call.await_reply(rx));
        assert!(flag.armed_at().is_some());
    }
}
