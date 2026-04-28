use async_nats::jetstream::consumer::{Consumer, pull};
use async_nats::jetstream::kv::Store;
use async_nats::jetstream::message::Message as JsMessage;

use super::bindings::wasmcloud::nats::types::NatsError;

/// Handle to a single JetStream-delivered message.
///
/// The inner message is wrapped in `Option` so ack/nak/term can consume it
/// exactly once via `take()`. Subsequent calls return an `unexpected` error.
/// `in-progress` only needs a shared reference and doesn't consume.
pub struct MessageHandle {
    pub(super) inner: Option<JsMessage>,
    pub(super) sequence: u64,
    pub(super) delivery_count: u32,
    pub(super) subject: String,
    pub(super) reply_to: Option<String>,
    pub(super) body: Vec<u8>,
    pub(super) headers: Option<async_nats::HeaderMap>,
}

/// Handle to a pull-based JetStream consumer.
///
/// The consumer is opened by the component and lives until the resource is
/// dropped.
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
