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

/// Generates the error classifiers over the generated `types` module.
///
/// A macro rather than plain functions because the handler worlds and the
/// imports world each generate their own `types`, so the classifiers are
/// expanded per module with `types` in scope rather than shared by import.
macro_rules! nats_error_classifiers {
    () => {
        /// Classifies a stream lookup, which must only claim `not-found` when
        /// the stream is genuinely absent.
        ///
        /// The WIT contract for `not-found` is "named resource doesn't exist",
        /// and a guest is entitled to act on that — recreate the consumer, take
        /// a permanent-failure path, alert that a bucket was deprovisioned. A
        /// timeout or a JetStream-disabled server answering the same way turns
        /// every transient outage into a destructive recovery.
        pub(super) fn stream_lookup_err(
            what: impl std::fmt::Display,
            e: async_nats::jetstream::context::GetStreamError,
        ) -> types::NatsError {
            classify_get_stream(what, &e)
        }

        fn classify_get_stream(
            what: impl std::fmt::Display,
            e: &async_nats::jetstream::context::GetStreamError,
        ) -> types::NatsError {
            use async_nats::jetstream::context::GetStreamErrorKind as Kind;
            match e.kind() {
                Kind::JetStream(err)
                    if err.error_code()
                        == async_nats::jetstream::ErrorCode::STREAM_NOT_FOUND =>
                {
                    types::NatsError::NotFound(format!("{what}: {e}"))
                }
                Kind::JetStream(_) => types::NatsError::Jetstream(format!("{what}: {e}")),
                // A caller-supplied name the client refused outright: nothing
                // by that name can exist.
                Kind::EmptyName | Kind::InvalidStreamName => {
                    types::NatsError::NotFound(format!("{what}: {e}"))
                }
                Kind::Request => request_stage_err(what, e),
            }
        }

        /// Classifies the transport stage of a JetStream API call by walking to
        /// the `RequestError` the outer error wraps.
        fn request_stage_err(
            what: impl std::fmt::Display,
            e: &(dyn std::error::Error + 'static),
        ) -> types::NatsError {
            use async_nats::jetstream::context::RequestErrorKind as Kind;
            let mut source = std::error::Error::source(e);
            while let Some(err) = source {
                if let Some(req) =
                    err.downcast_ref::<async_nats::jetstream::context::RequestError>()
                {
                    return match req.kind() {
                        Kind::TimedOut => types::NatsError::Timeout(format!(
                            "{what}: JetStream API request timed out"
                        )),
                        Kind::NoResponders => types::NatsError::Connection(format!(
                            "{what}: JetStream API unavailable (JetStream disabled or not responding)"
                        )),
                        _ => types::NatsError::Connection(format!("{what}: {req}")),
                    };
                }
                source = std::error::Error::source(err);
            }
            types::NatsError::Connection(format!("{what}: {e}"))
        }

        /// Classifies a KV bucket lookup, delegating to the stream classifier
        /// for the stream the bucket is backed by.
        pub(super) fn bucket_lookup_err(
            bucket: &str,
            e: async_nats::jetstream::context::KeyValueError,
        ) -> types::NatsError {
            use async_nats::jetstream::context::KeyValueErrorKind as Kind;
            let what = format!("bucket '{bucket}'");
            match e.kind() {
                Kind::InvalidStoreName => types::NatsError::NotFound(format!("{what}: {e}")),
                Kind::JetStream => types::NatsError::Jetstream(format!("{what}: {e}")),
                Kind::GetBucket => {
                    let mut source = std::error::Error::source(&e);
                    while let Some(err) = source {
                        if let Some(get) = err
                            .downcast_ref::<async_nats::jetstream::context::GetStreamError>()
                        {
                            return classify_get_stream(&what, get);
                        }
                        source = std::error::Error::source(err);
                    }
                    types::NatsError::NotFound(format!("{what}: {e}"))
                }
            }
        }

        /// Classifies a consumer lookup.
        ///
        /// `no-responders` is deliberately not reused for the JetStream API
        /// being unreachable: its WIT meaning is "nobody is subscribed to that
        /// subject", which is a guest-level fact about a core request.
        pub(super) fn consumer_lookup_err(
            stream: &str,
            consumer: &str,
            e: async_nats::jetstream::context::ConsumerInfoError,
        ) -> types::NatsError {
            use async_nats::jetstream::context::ConsumerInfoErrorKind as Kind;
            let what = format!("consumer '{consumer}' on stream '{stream}'");
            match e.kind() {
                Kind::NotFound | Kind::StreamNotFound => {
                    types::NatsError::NotFound(format!("{what}: {e}"))
                }
                Kind::InvalidName => types::NatsError::NotFound(format!("{what}: {e}")),
                Kind::TimedOut => {
                    types::NatsError::Timeout(format!("{what}: JetStream API request timed out"))
                }
                Kind::NoResponders => types::NatsError::Connection(format!(
                    "{what}: JetStream API unavailable (JetStream disabled or not responding)"
                )),
                Kind::Request => types::NatsError::Connection(format!("{what}: {e}")),
                Kind::JetStream(err)
                    if err.error_code() == async_nats::jetstream::ErrorCode::CONSUMER_NOT_FOUND
                        || err.error_code()
                            == async_nats::jetstream::ErrorCode::STREAM_NOT_FOUND =>
                {
                    types::NatsError::NotFound(format!("{what}: {e}"))
                }
                Kind::JetStream(_) | Kind::Offline => {
                    types::NatsError::Jetstream(format!("{what}: {e}"))
                }
            }
        }

        /// Classifies a JetStream publish, which carries three conditions the
        /// WIT already has variants for.
        ///
        /// Flattening them all into `jetstream(string)` makes a retry-safe ack
        /// timeout indistinguishable from a stream misconfiguration, so
        /// idempotent-republish logic keyed on `timeout` never fires.
        pub(super) fn js_publish_err(
            ctx: impl std::fmt::Display,
            e: async_nats::jetstream::context::PublishError,
            live_max_payload: u64,
        ) -> types::NatsError {
            use async_nats::jetstream::context::PublishErrorKind as Kind;
            match e.kind() {
                Kind::TimedOut => types::NatsError::Timeout(format!("{ctx}: {e}")),
                Kind::StreamNotFound => types::NatsError::NotFound(format!("{ctx}: {e}")),
                Kind::MaxPayloadExceeded => {
                    types::NatsError::MaxPayloadExceeded(live_max_payload)
                }
                // The CAS kinds have no WIT variant of their own; inventing one
                // here would be a contract change, not an error mapping.
                _ => types::NatsError::Jetstream(format!("{ctx}: {e}")),
            }
        }

        /// Classifies a core publish, whose only typed condition is oversize.
        pub(super) fn core_publish_err(
            ctx: impl std::fmt::Display,
            e: async_nats::client::PublishError,
            live_max_payload: u64,
        ) -> types::NatsError {
            match e.kind() {
                async_nats::client::PublishErrorKind::MaxPayloadExceeded => {
                    types::NatsError::MaxPayloadExceeded(live_max_payload)
                }
                _ => types::NatsError::Connection(format!("{ctx}: {e}")),
            }
        }

        /// Routes a KV operation failure to `timeout` or `jetstream`.
        ///
        /// The split is the whole reason both variants exist: a guest serving a
        /// cached value on `timeout` and alerting on `jetstream` would alert on
        /// every transient outage if both arrived as `jetstream`.
        pub(super) fn kv_err(
            ctx: impl std::fmt::Display,
            timed_out: bool,
            e: impl std::fmt::Display,
        ) -> types::NatsError {
            if timed_out {
                types::NatsError::Timeout(format!("{ctx}: {e}"))
            } else {
                types::NatsError::Jetstream(format!("{ctx}: {e}"))
            }
        }

        /// True when any error in the chain reports a `TimedOut` kind.
        ///
        /// `put`/`create` fold the timeout into an opaque publish stage, so the
        /// kind is not on the outermost error and has to be walked to.
        pub(super) fn chain_timed_out(e: &(dyn std::error::Error + 'static)) -> bool {
            use async_nats::jetstream::kv;
            let mut current = Some(e);
            while let Some(err) = current {
                if err
                    .downcast_ref::<kv::EntryError>()
                    .is_some_and(|e| matches!(e.kind(), kv::EntryErrorKind::TimedOut))
                    || err
                        .downcast_ref::<kv::UpdateError>()
                        .is_some_and(|e| matches!(e.kind(), kv::UpdateErrorKind::TimedOut))
                    || err
                        .downcast_ref::<kv::WatchError>()
                        .is_some_and(|e| matches!(e.kind(), kv::WatchErrorKind::TimedOut))
                    || err
                        .downcast_ref::<async_nats::jetstream::context::RequestError>()
                        .is_some_and(|e| {
                            matches!(
                                e.kind(),
                                async_nats::jetstream::context::RequestErrorKind::TimedOut
                            )
                        })
                {
                    return true;
                }
                current = std::error::Error::source(err);
            }
            false
        }
    };
}

pub(super) use nats_error_classifiers;
