use async_nats::jetstream::consumer::{Consumer, pull};
use async_nats::jetstream::kv::Store;
use async_nats::jetstream::message::Acker;

use super::bindings::wasmcloud::nats::types::NatsError;

/// Handle to a single JetStream-delivered message.
///
/// Holds an `Acker` rather than the whole message: `Message::split` separates
/// the ack capability from the contents, so the payload is not stored twice.
/// The acker is `take()`n on the first ack/nak/term, making the handle
/// one-shot; later calls report that it was already settled.
///
/// Under `ack-mode: auto` the host keeps the acker and this is `None`, so a
/// guest ack reports that the host owns the acknowledgement.
pub struct MessageHandle {
    pub(super) acker: Option<Acker>,
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

pub(super) fn jetstream_err(ctx: impl std::fmt::Display, e: impl std::fmt::Display) -> NatsError {
    NatsError::Jetstream(format!("{ctx}: {e}"))
}

/// Reported when a guest settles a message the host already owns.
pub(super) fn already_settled() -> NatsError {
    NatsError::Unexpected(
        "message already settled, or acknowledgement is owned by the host under ack-mode: auto"
            .to_string(),
    )
}
