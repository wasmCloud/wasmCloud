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
//! and `Continuity` shows guest code running without a pause. The last
//! separates the two kinds of stuck call — a guest waiting in a host call
//! stops executing, so the stretch resets, while a pinned one never does.
//! Abandonment alone would not do: dropping the dispatcher arms the flag, so
//! an ordinary client disconnect would condemn a store whose guest is healthy
//! and merely slow.
//!
//! The all-abandoned condition is what keeps the per-store blast radius safe:
//! continuity cannot tell a guest that yields every few hundred milliseconds
//! from a pinned one whose fires are spread that far by scheduler jitter, so
//! the trap waits until it would end nothing still wanted. Teardown of a
//! really pinned store is delayed by that, not lost — such a guest admits no
//! new calls, and the calls already on it are abandoned by their own
//! deadlines in turn.
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
pub struct AbandonFlag(AtomicU64);

impl AbandonFlag {
    fn new() -> Arc<Self> {
        Arc::new(Self(AtomicU64::new(0)))
    }

    /// Record the abandonment instant; only the first arming counts.
    pub(crate) fn arm(&self) {
        let _ = self
            .0
            .compare_exchange(0, now_millis(), Ordering::Relaxed, Ordering::Relaxed);
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
        match self.0.load(Ordering::Relaxed) {
            0 => None,
            at => Some(at),
        }
    }
}

/// Cancels a pending [`AbandonFlag::arm_after`] timer when dropped.
pub(crate) struct ArmTimer(#[expect(dead_code)] tokio_util::task::AbortOnDropHandle<()>);

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

    /// Whether any watched call has been abandoned for longer than `grace`.
    fn any_abandoned_longer_than(&self, grace: Duration) -> bool {
        let list = self.lock();
        // `now` is only worth computing once a flag is actually armed; the
        // common case (none are) stays free of clock reads.
        let mut now = None;
        let grace = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX);
        list.iter().any(|f| match f.armed_at() {
            None => false,
            Some(at) => {
                let now = *now.get_or_insert_with(now_millis);
                now.saturating_sub(at) >= grace
            }
        })
    }

    /// Whether nothing watched is abandoned, which re-arms the warning.
    fn none_abandoned(&self) -> bool {
        self.lock().iter().all(|f| f.armed_at().is_none())
    }

    /// Whether every watched call has been abandoned, so trapping the store
    /// would end nothing still wanted.
    fn all_abandoned(&self) -> bool {
        self.lock().iter().all(|f| f.armed_at().is_some())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Arc<AbandonFlag>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// How long guest code has been executing on one store without pausing.
///
/// The callback runs only while guest code does, so consecutive fires stay
/// about one sampling interval apart while a guest keeps executing, and a
/// wider gap means it stopped to wait for something. Measured here rather than
/// from the host side because the callback forces a yield on every fire, which
/// would refresh any host-side liveness signal and make a pinned guest look
/// healthy.
#[derive(Default)]
struct Continuity {
    /// When the current unbroken stretch of guest execution began.
    stretch_start: Option<u64>,
    /// When this callback last fired.
    last_fire: Option<u64>,
}

impl Continuity {
    /// Record a fire and return how long the guest has now been executing
    /// without a pause, in milliseconds.
    fn observe(&mut self, now: u64, pause_threshold_millis: u64) -> u64 {
        // A gap wider than the threshold means guest code stopped running in
        // between, so this starts a new stretch.
        let broken = self
            .last_fire
            .is_none_or(|last| now.saturating_sub(last) > pause_threshold_millis);
        if broken {
            self.stretch_start = Some(now);
        }
        self.last_fire = Some(now);
        now.saturating_sub(self.stretch_start.unwrap_or(now))
    }
}

/// How many sampling intervals a gap must exceed to count as a pause. Fires
/// are one interval apart while a guest runs straight through, so this leaves
/// room for scheduler jitter while staying well below any real wait.
const PAUSE_FACTOR: u32 = 5;

/// How long a gap between fires counts as the guest having paused, derived
/// from the sampling interval so it follows if that changes.
///
/// `WASH_ABANDONED_CALL_PAUSE_THRESHOLD_MS` overrides it, for a host loaded
/// enough that a pinned guest's fires land further apart than this — which
/// reads as a pause and stops anything from being trapped.
pub(crate) fn pause_threshold() -> Duration {
    static VALUE: LazyLock<Duration> = LazyLock::new(|| {
        let derived = crate::engine::EPOCH_TICK * (EPOCH_DEADLINE_TICKS as u32) * PAUSE_FACTOR;
        crate::timeouts::env_millis("WASH_ABANDONED_CALL_PAUSE_THRESHOLD_MS", derived)
    });
    *VALUE
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
/// place a pinned guest cannot avoid. Both conditions are required — an armed
/// flag alone only says nobody wants the result, which a client disconnect is
/// enough to cause. The module docs ([`crate::engine::abandon`]) cover it.
pub(crate) fn arm_epoch_deadline(
    store: &mut wasmtime::Store<SharedCtx>,
    policy: AbandonedCallPolicy,
) {
    let abandoned = Arc::clone(&store.data().abandoned);
    let store_id = Arc::clone(&store.data().active_ctx.store_id);
    let grace = crate::timeouts::abandoned_call_grace();
    let escalation = crate::timeouts::abandoned_call_escalation();
    let grace_millis = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX);
    let escalation_millis = u64::try_from(escalation.as_millis()).unwrap_or(u64::MAX);
    let pause_millis = u64::try_from(pause_threshold().as_millis()).unwrap_or(u64::MAX);
    let mut continuity = Continuity::default();
    // Reset below once nothing is abandoned, so a later wedge warns again.
    let mut warned = false;

    store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
    store.epoch_deadline_callback(move |_| {
        // Every fire counts, or the pattern means nothing.
        let pinned_for = continuity.observe(now_millis(), pause_millis);

        // Yield, never merely continue: this callback is the only point at
        // which a pinned guest hands the host back its thread. Ridden straight
        // through, the guest keeps the worker — and if that worker holds
        // tokio's time driver, *no timer on the runtime fires*, including the
        // very deadline whose expiry would abandon this call.
        let keep_running = |warned: &mut bool| {
            if abandoned.none_abandoned() {
                *warned = false;
            }
            Ok(wasmtime::UpdateDeadline::Yield(EPOCH_DEADLINE_TICKS))
        };

        if !abandoned.any_abandoned_longer_than(grace) {
            return keep_running(&mut warned);
        }
        // A call is still wanted: trapping the store would take it too. This
        // also protects a yielding guest whose wakes land closer together
        // than the pause threshold and so read as one pinned stretch — its
        // own wanted call is what keeps its store alive.
        if !abandoned.all_abandoned() {
            return keep_running(&mut warned);
        }
        // Abandoned, but the guest is still pausing to wait: slow, not wedged.
        if pinned_for < grace_millis {
            return keep_running(&mut warned);
        }
        if policy == AbandonedCallPolicy::WarnThenTrap && pinned_for < escalation_millis {
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
            pinned_for_ms = pinned_for,
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

        assert!(!calls.any_abandoned_longer_than(Duration::ZERO));
        flag.arm();
        assert!(calls.any_abandoned_longer_than(Duration::ZERO));
        drop(guard);
        assert!(!calls.any_abandoned_longer_than(Duration::ZERO));
    }

    #[test]
    fn a_wanted_call_blocks_all_abandoned() {
        let calls = Arc::new(AbandonedCalls::default());
        let given_up = DispatchedCall::new("test", Duration::ZERO);
        let wanted = DispatchedCall::new("test", Duration::ZERO);
        let _given_up_guard = calls.watch(given_up.flag());
        let wanted_guard = calls.watch(wanted.flag());

        given_up.flag().arm();
        assert!(calls.any_abandoned_longer_than(Duration::ZERO));
        assert!(!calls.all_abandoned());
        // The wanted call ends (or is abandoned in turn); nothing on the
        // store is wanted any more.
        drop(wanted_guard);
        assert!(calls.all_abandoned());
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
        assert!(!calls.any_abandoned_longer_than(Duration::from_secs(3600)));
        // Re-arming must not push the abandonment instant forward.
        flag.arm();
        assert!(calls.any_abandoned_longer_than(Duration::ZERO));
    }

    /// A pause between fires starts a new stretch; unbroken fires accumulate.
    #[test]
    fn continuity_resets_on_a_pause_and_accumulates_otherwise() {
        const T: u64 = 500;
        let mut c = Continuity::default();
        // A run of fires one sampling interval apart: one unbroken stretch.
        assert_eq!(c.observe(1_000, T), 0);
        assert_eq!(c.observe(1_100, T), 100);
        assert_eq!(c.observe(1_200, T), 200);
        // A gap past the threshold — the guest stopped running and waited, so
        // the next fire begins a fresh stretch however long the last one was.
        let after_pause = 1_200 + T + 1;
        assert_eq!(c.observe(after_pause, T), 0);
        assert_eq!(c.observe(after_pause + 100, T), 100);
        // A gap *at* the threshold is still the same stretch: only a wider one
        // counts as a pause, so scheduler jitter cannot reset it.
        let jittered = after_pause + 100 + T;
        assert_eq!(c.observe(jittered, T), jittered - after_pause);
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
