//! Host-side handles behind the JetStream resources: a delivered message, a
//! pull consumer, and an open KV bucket. The `with:` mappings in [`super`] and
//! [`super::interfaces`] point the generated resource types at these.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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
}

/// Handle to a pull-based JetStream consumer.
pub struct PullConsumerHandle {
    pub(super) consumer: Option<Consumer<pull::Config>>,
}

/// Handle to an open JetStream KV bucket.
pub struct BucketHandle {
    pub(super) store: Store,
}
