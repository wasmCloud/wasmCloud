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
//! The callback acts only on a call that has stayed abandoned *and still
//! registered* for longer than [`crate::timeouts::abandoned_call_grace`]. The
//! grace is what makes abandonment safe to signal eagerly: a healthy guest
//! finishes the abandoned call and deregisters it well inside the grace, so a
//! client disconnect costs nothing, while a wedged call is still there when
//! the grace runs out. Without it, every mid-call disconnect would condemn a
//! store that was serving other callers perfectly well.
//!
//! No judgement of the guest is made anywhere here. The epoch advances on wall
//! clock, so it can report whether guest code is running, never whether it is
//! getting anywhere: a store pegged at 100% CPU serving calls its callers
//! still want is left alone. The one decision — "nobody wants this any more" —
//! is the dispatcher's, and it is one the host already makes today by dropping
//! futures; this module only makes it stick against a guest that never yields.
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

    /// Arm this flag when `deadline` elapses, from a detached timer. For
    /// re-bounding a call that outlives its reply (a post-reply stream drain),
    /// and for waiters that themselves live inside the dispatched-to store. A
    /// call already deregistered by then makes the arming invisible.
    pub(crate) fn arm_after(self: Arc<Self>, deadline: Duration) {
        tokio::spawn(async move {
            tokio::time::sleep(deadline).await;
            self.arm();
        });
    }

    fn armed_at(&self) -> Option<u64> {
        match self.0.load(Ordering::Relaxed) {
            0 => None,
            at => Some(at),
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

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Arc<AbandonFlag>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
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
    /// call and arm the flag from a detached timer at the deadline instead.
    #[cfg_attr(not(feature = "host-component-plugins"), allow(dead_code))]
    pub(crate) fn arm_detached(self) {
        let Self {
            watch, deadline, ..
        } = self;
        watch.flag().arm_after(deadline);
        watch.disarm();
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

/// How many epoch ticks between deadline checks — how promptly an abandoned
/// call is noticed once its grace runs out, and how often a busy guest yields
/// its worker thread back to the executor.
///
/// Neither purpose wants this fine. Noticing an armed flag within ~100ms is
/// ample against deadlines measured in seconds, and yielding every ~100ms of
/// continuous guest execution is enough to keep a spinning guest from
/// starving the executor (and tokio's time driver) for longer than that. What
/// rules out a *finer* setting is the yield itself: it requeues the store's
/// task through the executor, and paying that on every 10ms tick showed up as
/// a measurable per-request cost on the p3 request path — ~2% of requests
/// cross a tick mid-guest and eat a ~1ms reschedule.
const EPOCH_DEADLINE_TICKS: u64 = 10;

/// What a store does about a call abandoned past its grace; see
/// [`arm_epoch_deadline`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AbandonedCallPolicy {
    /// Trap the store at the grace. For a store that can be replaced: an
    /// ephemeral store serves one call, a pooled instance is reaped and
    /// rebuilt on the next call, and a trapped service is restarted by its
    /// supervisor. Everything sharing the store goes down with it — the same
    /// blast radius as a guest trap, bounded the same way.
    Trap,
    /// Warn at the grace, trap only at
    /// [`crate::timeouts::abandoned_call_escalation`]. For a host component
    /// plugin's store, which serves every tenant at once: the long runway lets
    /// an abandoned call that still *yields* finish harmlessly, costing no one
    /// anything. A call that never yields, though, holds the store's guest
    /// execution for as long as it lives — no other tenant's call can enter —
    /// so a store still carrying it at the escalation is already down for
    /// everyone, and trapping it is what brings it back (the supervisor
    /// rebuilds it and replays binds, the same path an organic plugin trap
    /// takes). Per-task cancellation (bytecodealliance/wasmtime#11833) is what
    /// would end the one call instead.
    WarnThenTrap,
}

/// Let the host end guest work whose caller has abandoned it, even if the
/// guest never yields.
///
/// Every store needs this call whether or not anything will ever abandon a
/// call on it — a store that never sets a deadline traps the moment it runs
/// any guest code, and the host cannot install one later.
///
/// The callback is the enforcement point because it is the only one left: a
/// guest that never yields never returns from its poll, so every host future
/// on that store — timeouts included — is stuck behind it. wasmtime compiles
/// the deadline check into the guest's own loop back-edges, which is the one
/// place a spinning guest cannot avoid. The module docs
/// ([`crate::engine::abandon`]) cover the model.
pub(crate) fn arm_epoch_deadline(
    store: &mut wasmtime::Store<SharedCtx>,
    policy: AbandonedCallPolicy,
) {
    let abandoned = Arc::clone(&store.data().abandoned);
    let store_id = Arc::clone(&store.data().active_ctx.store_id);
    let grace = crate::timeouts::abandoned_call_grace();
    let escalation = crate::timeouts::abandoned_call_escalation();
    let mut warned = false;

    store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
    store.epoch_deadline_callback(move |_| {
        if !abandoned.any_abandoned_longer_than(grace) {
            // Yield, never merely continue: this callback is the only point at
            // which a non-yielding guest hands the host back its thread. Ridden
            // straight through, the guest keeps the worker — and if that worker
            // holds tokio's time driver, *no timer on the runtime fires*,
            // including the very deadline whose expiry would abandon this call.
            // The yield is what keeps the enforcement loop closed.
            return Ok(wasmtime::UpdateDeadline::Yield(EPOCH_DEADLINE_TICKS));
        }
        if policy == AbandonedCallPolicy::WarnThenTrap
            && !abandoned.any_abandoned_longer_than(escalation)
        {
            if !warned {
                warned = true;
                tracing::warn!(
                    store_id = %store_id,
                    escalation = ?escalation,
                    "a call on this store was abandoned past its grace but its guest is still \
                     running; the store is shared, so it is not trapped before the escalation"
                );
            }
            return Ok(wasmtime::UpdateDeadline::Yield(EPOCH_DEADLINE_TICKS));
        }
        tracing::error!(
            store_id = %store_id,
            "a call on this store was abandoned but its guest is still running; trapping the store"
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
