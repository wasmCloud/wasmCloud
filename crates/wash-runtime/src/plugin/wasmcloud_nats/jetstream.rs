//! Host-side handles behind the JetStream resources: a delivered message, a
//! pull consumer, and an open KV bucket. The `with:` mappings in [`super`] and
//! [`super::interfaces`] point the generated resource types at these.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_nats::jetstream::consumer::{Consumer, pull};
use async_nats::jetstream::kv::Store;
use async_nats::jetstream::message::Acker;

/// Handle to a single JetStream-delivered message.
///
/// Holds an `Acker` rather than the whole message: `Message::split` separates
/// the ack capability from the contents, so the payload is not stored twice.
/// The acker is `take()`n on the first ack/nak/term the server accepts, making
/// the handle one-shot; later calls report that it was already settled. A
/// settle that failed on the wire is retryable.
///
/// Under `ack-mode: auto` the host keeps the acker and this is `None`, so a
/// guest ack reports that the host owns the acknowledgement.
pub struct MessageHandle {
    /// Settlement ownership: present when the guest is the one that settles.
    ///
    /// Cleared on the first *successful* settle, making the handle one-shot. A
    /// settle the server rejected leaves it in place, so the guest can retry
    /// the natural response to a transient error rather than being told the
    /// message was already settled.
    ///
    /// Under `ack-mode: auto` the host settles, so this is `None`.
    pub(super) acker: Option<Arc<Acker>>,
    /// Ack-wait extension, which settles nothing.
    ///
    /// Present whenever the delivery has an acker at all, in *both* ack modes:
    /// `in-progress` is the WIT-sanctioned way for a slow handler to keep a
    /// message from being redelivered underneath it, and gating it on
    /// settlement ownership left auto — the default mode — with no way to do
    /// that at all.
    pub(super) progress: Option<Arc<Acker>>,
    /// Set when the message has been settled in a way that retires it: an
    /// `ack`, or a `term` that deliberately discards it. A `nak`, a settle the
    /// server rejected, and a handler that returns without settling all leave
    /// it clear, so the push subscriber keeps the sequence in its in-flight set
    /// and a consumer rebuilt after a reconnect replays it.
    ///
    /// A plain `ack` is a fire-and-forget publish, so this records that the ack
    /// was written, not that the server took it: an ack lost in a disconnect
    /// can still let a rebuild resume past the message. A guest that cannot
    /// tolerate that has `ack-sync`, which waits for the server.
    ///
    /// Ignored for pull-consumer messages: those are guest-driven, and the host
    /// keeps no resume point for them.
    pub(super) settled: Arc<AtomicBool>,
    pub(super) message: async_nats::Message,
    pub(super) sequence: u64,
    pub(super) delivery_count: u32,
    /// The binding budget this delivery is charged against, and what it was
    /// charged, released when the handle is dropped.
    ///
    /// Only a pull-fetched delivery carries one: a push delivery is already
    /// bounded by `max-ack-pending`, and its handle lives and dies with the
    /// store the host built for it.
    pub(super) charged: Option<(Arc<FetchBudget>, u64)>,
}

impl Drop for MessageHandle {
    fn drop(&mut self) {
        // Dropping the handle is what actually frees the payload, so it is
        // also what returns its bytes to the binding's budget. Doing this in
        // `Drop` rather than at the resource-drop call site covers every way a
        // handle can go: a guest drop, a table teardown, a torn-down workload.
        if let Some((budget, bytes)) = self.charged.take() {
            budget.release(bytes);
        }
    }
}

/// Handle to a pull-based JetStream consumer.
pub struct PullConsumerHandle {
    pub(super) consumer: Option<Consumer<pull::Config>>,
    /// Ceiling on what one `fetch` may materialize into host memory, taken
    /// from the binding that opened it.
    ///
    /// `fetch` names a message count and nothing else, so `fetch(100)` against
    /// a stream of 5 MiB messages asks the host to hold 500 MiB — which
    /// OOM-killed it. The count is the guest's; this bound is the binding's,
    /// and the smaller of the two wins.
    pub(super) max_fetch_bytes: u64,
    /// The binding's running total, which is what bounds a guest that loops
    /// `fetch` — the ordinary shape of a pull worker. See [`FetchBudget`].
    pub(super) budget: Arc<FetchBudget>,
    /// The grant every delivery is checked against, from the binding that
    /// opened the consumer. A durable is provisioned out of band, so its filter
    /// is not this workload's to trust: `open` refuses one that reaches outside
    /// the grant, and this catches a filter widened after the attach.
    pub(super) policy: Arc<super::policy::PolicyEngine>,
}

/// What a binding's pull consumers are holding in host memory right now.
///
/// Bounding one `fetch` bounds one call and nothing else: a worker that loops
/// `fetch` until drained walks the host into an OOM at a rate set only by
/// message size, because a delivered message stays resident until the guest
/// drops its handle — acking does not free it, and a guest that never drops
/// never frees it at all. This is the budget that spans fetches: every
/// delivery is charged on the way out and released when its handle is dropped,
/// and a `fetch` may only ask for what is left.
#[derive(Debug)]
pub struct FetchBudget {
    outstanding: AtomicU64,
    ceiling: u64,
}

impl FetchBudget {
    pub(super) fn new(ceiling: u64) -> Self {
        Self {
            outstanding: AtomicU64::new(0),
            // A ceiling of zero would refuse every fetch; treat it as "one
            // message at a time" rather than "nothing ever".
            ceiling: ceiling.max(1),
        }
    }

    /// Bytes a fetch may still materialize.
    pub(super) fn available(&self) -> u64 {
        self.ceiling
            .saturating_sub(self.outstanding.load(Ordering::Relaxed))
    }

    /// The whole budget, for the error that has to name it.
    pub(super) fn ceiling(&self) -> u64 {
        self.ceiling
    }

    pub(super) fn outstanding(&self) -> u64 {
        self.outstanding.load(Ordering::Relaxed)
    }

    pub(super) fn charge(&self, bytes: u64) {
        self.outstanding.fetch_add(bytes, Ordering::Relaxed);
    }

    pub(super) fn release(&self, bytes: u64) {
        // Saturating: a double release would otherwise wrap the counter and
        // hand the binding an unbounded budget for the rest of its life.
        let _ = self
            .outstanding
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |held| {
                Some(held.saturating_sub(bytes))
            });
    }
}

/// Handle to an open JetStream KV bucket.
pub struct BucketHandle {
    pub(super) store: Store,
    /// The connection `open` routed to, and the limits a write is checked
    /// against.
    ///
    /// Carried rather than looked up per call: a labeled (`(implements ..)`)
    /// import resolves its connection from the `NatsId` the call arrives with,
    /// and a resource method has no id to resolve. Looking it up by workload
    /// found the *unnamed* binding instead — an error where a labeled-only
    /// workload has none, and the wrong server's `max_payload` where it does.
    pub(super) conn: Arc<super::conn::ConnHandle>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_spans_fetches_and_is_released_by_dropping_a_handle() {
        // The shape that OOM-killed the host: a worker looping `fetch`, acking
        // every message, never dropping a handle. The budget has to notice.
        let budget = Arc::new(FetchBudget::new(30));
        assert_eq!(budget.available(), 30);
        budget.charge(25);
        assert_eq!(budget.available(), 5);
        budget.charge(5);
        assert_eq!(
            budget.available(),
            0,
            "a full budget must refuse the next fetch rather than let it OOM the host"
        );
        budget.release(25);
        assert_eq!(budget.available(), 25);
    }

    #[test]
    fn a_double_release_cannot_hand_out_an_unbounded_budget() {
        let budget = FetchBudget::new(100);
        budget.charge(10);
        budget.release(10);
        budget.release(10);
        assert_eq!(budget.outstanding(), 0);
        assert_eq!(budget.available(), 100);
    }

    #[test]
    fn a_zero_ceiling_still_admits_one_message_at_a_time() {
        assert_eq!(FetchBudget::new(0).ceiling(), 1);
    }
}
