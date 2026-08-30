//! The `wasmcloud:messaging` host plugin, over NATS or an in-memory broker.
//!
//! # Per-component configuration
//!
//! Every knob below is read from the component's `LocalResources.config` —
//! `dev.components[].config` in a `wash dev` config, or `localResources.config`
//! on a workload — falling back to the workload-scoped `wasmcloud:messaging`
//! host-interface config where the backend supports it. All are optional.
//!
//! | Key | Value | Unset |
//! | --- | --- | --- |
//! | `subscriptions` | Comma-separated subjects, NATS wildcards allowed (`orders.*`, `audit.>`) | Receive everything |
//! | `consumer_group` | Queue-group name, or `broadcast` for no grouping (NATS only) | A name derived from namespace/workload/component |
//! | `max_in_flight` | Messages this component may process at once, across every replica of it on this host | The host's per-component default |
//! | `admission_wait` | How long to wait for a slot before shedding (`45s`, `2m`, or bare seconds) | [`DEFAULT_ADMISSION_WAIT`] |
//!
//! ```yaml
//! localResources:
//!   config:
//!     subscriptions: "orders.*.created,audit.>"
//!     max_in_flight: "64"
//!     admission_wait: "5m"     # this handler is slow on purpose
//! ```
//!
//! # Instance limits
//!
//! A component exporting the async `wasmcloud:messaging/handler@0.3.0` serves
//! its deliveries on the workload's instance pool, so the `poolSize`,
//! `maxInvocations` and `maxConcurrency` it declares mean here what they mean
//! for inbound HTTP and linked calls — see [`crate::engine::instance_driver`].
//! A component exporting the sync `@0.2.0` handler cannot: its call holds the
//! store for its whole length, so it keeps an instance per message.
//!
//! `max_in_flight` is a **per-component total**, unlike `max_concurrency`
//! (per warm instance), and is separately bounded by the host-wide ceiling —
//! see [`MessagingLimits`]. It is a total for the *component*, not for one
//! replica of it: replicas of a deployment that land on the same host share
//! one ceiling rather than getting one apiece, so `replicas: 4` with
//! `max_in_flight: "32"` admits 32 messages on a host, not 128. `admission_wait`
//! is per component rather than per host because the right answer depends on
//! the handler: minutes-long work wants to queue, interactive work wants to
//! shed and stay responsive.

pub(crate) mod in_memory;
#[cfg(feature = "wasm_component_model_implements")]
pub(crate) mod multiplexed;
#[cfg(feature = "wasm_component_model_implements")]
pub(crate) mod multiplexed_async;
pub(crate) mod nats;

use std::sync::Arc;
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::metrics::Counter;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub use in_memory::InMemoryMessaging;
#[cfg(feature = "wasm_component_model_implements")]
pub use multiplexed::{
    BrokerMessage, InMemoryMsgBackend, InMemoryMsgProvider, MsgBackend, MsgId, MsgProvider,
    MultiplexedMessaging, NatsMsgBackend, NatsMsgProvider,
};
#[cfg(feature = "wasm_component_model_implements")]
pub use multiplexed_async::MultiplexedAsyncMessaging;
pub use nats::NatsMessaging;

/// A messaging failure, classified into the named cases of the async
/// `wasmcloud:messaging@0.3.0` `error` variant.
///
/// The two messaging surfaces disagree on how an error is spelled: `@0.2.0`
/// returns a bare `string`, `@0.3.0` a non-exhaustive variant. Producers of an
/// error therefore classify once, into this type, and each host impl lowers it —
/// to a string via [`Display`](std::fmt::Display) for the sync path, into the WIT
/// variant for the async one (see `multiplexed_async`).
///
/// Every case carries the human-readable detail. The sync path prints it, so its
/// error strings are unchanged from before this type existed; the async path
/// keeps it only for [`MsgError::Other`], since the WIT's named cases have no
/// payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsgError {
    /// The subject is malformed, empty, or otherwise rejected by the broker.
    SubjectInvalid(String),
    /// The component is not permitted to publish to this subject.
    AccessDenied(String),
    /// A `request` did not receive a reply within its `timeout_ms`.
    Timeout(String),
    /// The broker is unreachable or otherwise unavailable.
    BrokerUnavailable(String),
    /// The message body exceeded the broker's maximum payload size.
    MessageTooLarge(String),
    /// A quota or rate limit was exceeded.
    QuotaExceeded(String),
    /// A backend-specific failure that maps to no named case above.
    Other(String),
}

impl std::fmt::Display for MsgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            MsgError::SubjectInvalid(d)
            | MsgError::AccessDenied(d)
            | MsgError::Timeout(d)
            | MsgError::BrokerUnavailable(d)
            | MsgError::MessageTooLarge(d)
            | MsgError::QuotaExceeded(d)
            | MsgError::Other(d) => d,
        };
        f.write_str(detail)
    }
}

impl std::error::Error for MsgError {}

/// The lowering used by the sync `@0.2.0` surface, whose WIT `error` is a `string`.
impl From<MsgError> for String {
    fn from(e: MsgError) -> Self {
        e.to_string()
    }
}

/// Expands the [`MsgError`] lowering into one `bindgen!`-generated `@0.3.0`
/// `error` type.
///
/// Each messaging plugin has its own `bindgen!` invocation — they implement the
/// generated host traits for different backends, so they cannot share one — and
/// each therefore gets its own `error` Rust type even though the WIT is a single
/// package. The lowering is identical in every case, so it is written once here
/// and expanded per module rather than copied three times and left to drift.
///
/// Note this is the only capability that needs such a thing, which is why no
/// sibling has one: `wasmcloud:keyvalue` and `wasmcloud:blobstore` asyncified
/// only their *multiplexed* plugin, leaving one async `bindgen!` apiece and so
/// nothing to share (keyvalue's exported `watcher` is declared in WIT but never
/// served host-side). Messaging asyncifies all three plugins, because its
/// exported `handler` — the subscription path, served by the standalone NATS and
/// in-memory plugins that `wash dev` and `wash host` register — is the half of
/// the capability that receives messages.
///
/// The named WIT cases carry no payload, so their host-side detail is dropped by
/// the lowering; it survives only on `other`.
macro_rules! async_messaging_conversions {
    (error: $async_error:ty $(,)?) => {
        impl From<$crate::plugin::wasmcloud_messaging::MsgError> for $async_error {
            fn from(e: $crate::plugin::wasmcloud_messaging::MsgError) -> Self {
                use $crate::plugin::wasmcloud_messaging::MsgError as E;
                match e {
                    E::SubjectInvalid(_) => Self::SubjectInvalid,
                    E::AccessDenied(_) => Self::AccessDenied,
                    E::Timeout(_) => Self::Timeout,
                    E::BrokerUnavailable(_) => Self::BrokerUnavailable,
                    E::MessageTooLarge(_) => Self::MessageTooLarge,
                    E::QuotaExceeded(_) => Self::QuotaExceeded,
                    E::Other(d) => Self::Other(d),
                }
            }
        }
    };
}

pub(crate) use async_messaging_conversions;

/// Expands `render_handle_error`, which renders one `bindgen!`-generated
/// `@0.3.0` `handle-message-error` for the host's ack/log path: the payload-less
/// dispositions as their case name, `other` keeping its detail.
///
/// Same reason as [`async_messaging_conversions`] — every module binding the
/// handler gets its own generated disposition type — but a separate macro
/// because the two are needed in different places: the multiplexed plugin
/// lowers errors without ever invoking a handler, and the trigger service
/// invokes a handler without lowering errors.
///
/// The rendered strings are a host-observable contract (the delivery outcome a
/// backend logs, and what the trigger-service tests assert on), so every path
/// that reports a disposition has to spell it the same way.
macro_rules! messaging_disposition_rendering {
    (disposition: $disposition:ty $(,)?) => {
        fn render_handle_error(e: $disposition) -> String {
            // Aliased because a variant cannot be named through a qualified
            // path in a pattern, which is all an interpolated type is.
            type Disposition = $disposition;
            match e {
                Disposition::Reject => "reject".to_string(),
                Disposition::Retry => "retry".to_string(),
                Disposition::Other(d) => format!("other: {d}"),
            }
        }
    };
}

pub(crate) use messaging_disposition_rendering;

/// Drain a `@0.3.0` message body (`stream<u8>`) into memory.
///
/// The `@0.3.0` WIT carries bodies as native streams, but every current backend
/// sends a message as one bounded payload (core NATS caps it at `max_payload`),
/// so the host consumes the guest's stream fully before handing bytes to the
/// backend — the buffered fallback the WIT documents. A backend that can
/// forward a stream incrementally can bypass this helper.
///
/// One helper serves every plugin because [`StreamReader`] is a wasmtime type,
/// not a `bindgen!`-generated one; it wraps the shared
/// [`collect_stream`](crate::plugin::stream_collect::collect_stream) with
/// messaging's cap and error vocabulary.
///
/// [`StreamReader`]: wasmtime::component::StreamReader
pub(crate) async fn collect_body<T, D>(
    accessor: &wasmtime::component::Accessor<T, D>,
    body: wasmtime::component::StreamReader<u8>,
) -> wasmtime::Result<Result<Vec<u8>, MsgError>>
where
    T: 'static,
    D: wasmtime::component::HasData,
{
    use crate::plugin::stream_collect::{CollectError, collect_stream};
    Ok(collect_stream(accessor, body, MAX_COLLECTED_BODY_BYTES)
        .await?
        .map_err(|e| match e {
            // The one collection failure with a named WIT case: the guest
            // wrote a body no broker would have taken anyway.
            e @ CollectError::LimitExceeded { .. } => MsgError::MessageTooLarge(e.to_string()),
            e @ CollectError::Abandoned => MsgError::Other(e.to_string()),
        }))
}

/// Upper bound on a collected message body, bounding host memory against a
/// guest that streams unboundedly. Deliberately above any sane broker payload
/// limit (NATS `max_payload` defaults to 1 MiB and tops out well below this),
/// so the broker's own limit stays the effective one for sendable messages and
/// this cap only stops runaway streams.
const MAX_COLLECTED_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Report one delivery's outcome.
///
/// The handler's own error is the useful one; the outer result carries traps
/// and the host failures that kept the delivery from reaching the guest. Both
/// backends and both delivery shapes report through this, so a message that
/// failed reads the same however it was served.
pub(crate) fn log_delivery(result: &anyhow::Result<Result<(), String>>) {
    match result {
        Ok(Ok(())) => tracing::debug!("message handled successfully"),
        Ok(Err(handler_err)) => {
            tracing::warn!("messaging handler returned an error: {handler_err}")
        }
        Err(e) => tracing::warn!("message delivery failed: {e:#}"),
    }
}

/// Runs one `@0.3.0` delivery: through the workload's instance pool when the
/// component opted into pooling, and in a store of its own when every warm
/// instance is busy.
///
/// The job crosses as [`InstanceJob::Messaging`], so a message takes the same
/// path as inbound HTTP and linked calls and honours the same `poolSize`,
/// `maxInvocations` and `maxConcurrency` the component declared — including
/// several deliveries in flight on one instance, which per-message stores can
/// never do.
///
/// The same job runs either way: a delivery the pool declines is not rebuilt,
/// it is handed back and run in a fresh store, so a large payload is never
/// copied to pay for the attempt.
///
/// Only `@0.3.0` reaches here. The sync `@0.2.0` handler takes `&mut Store` for
/// the length of its call, so it cannot share an instance and keeps a store per
/// message.
pub(crate) async fn deliver_pooled(
    workload: &crate::engine::workload::ResolvedWorkload,
    component_id: &str,
    pre: &wasmtime::component::InstancePre<crate::engine::ctx::SharedCtx>,
    pool: &Arc<crate::engine::instance_pool::InstancePool>,
    msg: crate::host::trigger_service::BrokerMessage,
    attributes: Vec<KeyValue>,
) -> anyhow::Result<Result<(), String>> {
    use crate::engine::instance_driver::InstanceJob;
    use crate::engine::instance_pool::{ComponentInstance, Dispatch};
    use crate::host::trigger_service::{MessagingJob, MessagingTask};

    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    // Enforced out here, outside the store the guest runs in, so a guest that
    // never yields cannot hold off its own deadline.
    let call = crate::engine::abandon::DispatchedCall::new(
        "messaging (pooled)",
        crate::timeouts::messaging_deliver(),
    );
    let job = MessagingJob {
        msg,
        result_tx,
        abandoned: call.flag(),
        attributes,
    };

    let declined = match pool.try_dispatch(InstanceJob::Messaging(Box::new(job))) {
        Dispatch::Sent => None,
        // Built out here, where awaiting is allowed and where a component that
        // fails to instantiate reports it to this delivery rather than only to
        // the log.
        Dispatch::NeedsInstance(job) => {
            let mut store = workload.new_store(component_id).await?;
            let instance = pre.instantiate_async(&mut store).await?;
            pool.dispatch_on_new(ComponentInstance { store, instance }, job)
                .err()
        }
        Dispatch::Saturated(job) => Some(job),
    };

    let Some(declined) = declined else {
        return call
            .await_reply(result_rx)
            .await
            .ok_or_else(|| anyhow::anyhow!("pooled instance produced no message response in time"))?
            .map_err(|_| anyhow::anyhow!("pooled instance dropped the message response"));
    };

    // Every instance was busy and the pool is full, so run the very same job in
    // a store of its own.
    let InstanceJob::Messaging(job) = declined else {
        anyhow::bail!("instance pool returned the wrong job kind for a message");
    };
    tracing::debug!(component_id, "warm instances saturated; own store");

    let mut store = workload.new_store(component_id).await?;
    let instance = pre.instantiate_async(&mut store).await?;
    let handler = Arc::new(
        crate::host::trigger_service::AsyncMessaging::new(&mut store, &instance).map_err(|e| {
            anyhow::anyhow!("component does not export wasmcloud:messaging/handler@0.3.0: {e:#}")
        })?,
    );
    let MessagingJob {
        msg,
        result_tx,
        abandoned,
        attributes,
    } = *job;
    // Awaited through the same `DispatchedCall` the pooled path uses: `arm_after`
    // runs inside `await_reply`, so a cold delivery that skipped it would be
    // unbounded while looking bounded. No pool slot — the store goes when this
    // future is dropped, which is what ends the guest's work.
    let task = MessagingTask {
        handler,
        msg,
        result_tx,
        abandoned,
        attributes,
        pool_slot: None,
    };
    // Two unwraps, not one: the outer is `run_concurrent` faulting, the inner
    // the task's own trap.
    call.await_reply(store.run_concurrent(async move |accessor| {
        wasmtime::component::AccessorTask::run(task, accessor).await
    }))
    .await
    .ok_or_else(|| anyhow::anyhow!("delivery produced no message response in time"))?
    .map_err(|e| anyhow::anyhow!("{e:#}"))?
    .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    result_rx
        .await
        .map_err(|_| anyhow::anyhow!("delivery task dropped the message response"))
}

/// The WIT export a delivery invokes, and what its measurements are grouped by.
/// Bounded by the interface set, so it is safe as a metric attribute — unlike
/// the concrete subject, which stays on the span and the logs.
///
/// One string for every backend and both delivery shapes, so a dashboard reads
/// a service's deliveries and a pooled component's as the same operation.
pub(crate) const MESSAGING_OPERATION: &str = "wasmcloud:messaging/handler#handle-message";

/// Mint a `stream<u8>` carrying `bytes`, for handing a message body to a guest.
pub(crate) fn mint_body<T, D>(
    accessor: &wasmtime::component::Accessor<T, D>,
    bytes: Vec<u8>,
) -> wasmtime::Result<wasmtime::component::StreamReader<u8>>
where
    T: 'static,
    D: wasmtime::component::HasData,
{
    accessor.with(|mut a| wasmtime::component::StreamReader::new(&mut a, bytes))
}

/// Expands the per-message handler dispatch shared by the standalone plugins.
///
/// A subscriber loop pre-instantiates the component once and then invokes its
/// `handle-message` export per message. Which export that is depends on the
/// revision the guest was built against, so this generates a two-variant
/// `HandlerPre`/`HandlerProxy` pair that resolves the revision once, at
/// pre-instantiation, and dispatches on it thereafter.
///
/// Both standalone plugins need exactly this over their own `bindgen!` types
/// (see [`async_messaging_conversions`] for why the types are per-module), so it
/// is written once here.
macro_rules! messaging_handler_dispatch {
    (sync: $sync:ident, async: $async:ident $(,)?) => {
        /// A component's messaging handler, resolved once where the
        /// subscription is set up rather than once per message.
        ///
        /// Both halves come from the same `InstancePre`: `pre` is the
        /// revision-typed view the per-message store path calls through, and
        /// `instance_pre` is what the instance pool instantiates a warm
        /// instance from — the pool binds its own typed view.
        struct HandlerTarget {
            handler: HandlerPre,
            instance_pre: wasmtime::component::InstancePre<$crate::engine::ctx::SharedCtx>,
        }

        impl HandlerTarget {
            fn new(
                instance_pre: wasmtime::component::InstancePre<$crate::engine::ctx::SharedCtx>,
            ) -> wasmtime::Result<Self> {
                Ok(Self {
                    handler: HandlerPre::new(instance_pre.clone())?,
                    instance_pre,
                })
            }

            /// Whether this component exports the async `@0.3.0` handler, the
            /// only revision a pooled instance can serve: its call takes an
            /// `Accessor`, so deliveries share one instance up to
            /// `max_concurrency`. The sync `@0.2.0` export holds `&mut Store`
            /// for the length of its call and keeps its per-message store.
            fn serves_async(&self) -> bool {
                matches!(self.handler, HandlerPre::V0_3(_))
            }

            /// What the instance pool instantiates a warm instance from. The
            /// pool binds its own typed view over the result, so this is the
            /// raw pre rather than the revision-typed one.
            fn instance_pre(
                &self,
            ) -> &wasmtime::component::InstancePre<$crate::engine::ctx::SharedCtx> {
                &self.instance_pre
            }

            /// Instantiate for one message, on the per-message store path.
            async fn instantiate(
                &self,
                store: &mut wasmtime::Store<$crate::engine::ctx::SharedCtx>,
            ) -> wasmtime::Result<HandlerProxy> {
                self.handler.instantiate(store).await
            }
        }

        /// A pre-instantiated messaging handler component, at whichever revision
        /// of `wasmcloud:messaging/handler` it exports.
        enum HandlerPre {
            V0_2($sync::MessagingPre<$crate::engine::ctx::SharedCtx>),
            V0_3($async::AsyncMessagingPre<$crate::engine::ctx::SharedCtx>),
        }

        /// An instantiated handler, ready to receive one message.
        enum HandlerProxy {
            V0_2($sync::Messaging),
            V0_3($async::AsyncMessaging),
        }

        impl HandlerPre {
            /// Resolve the exported handler revision, preferring the async
            /// `@0.3.0` one. `MessagingPre::new` is what actually type-checks the
            /// export against a revision, so trying `@0.3.0` first and falling
            /// back is both the check and the selection.
            fn new(
                instance_pre: wasmtime::component::InstancePre<$crate::engine::ctx::SharedCtx>,
            ) -> wasmtime::Result<Self> {
                match $async::AsyncMessagingPre::new(instance_pre.clone()) {
                    Ok(p) => {
                        // Dual export: `@0.3.0` wins; warn so the dead `@0.2.0`
                        // export is visible rather than silently ignored.
                        if $sync::MessagingPre::new(instance_pre).is_ok() {
                            tracing::warn!(
                                served = "wasmcloud:messaging/handler@0.3.0",
                                ignored = "wasmcloud:messaging/handler@0.2.0",
                                "component exports both messaging handler revisions; only \
                                 the @0.3.0 handler will be invoked — export exactly one \
                                 messaging handler revision"
                            );
                        }
                        Ok(HandlerPre::V0_3(p))
                    }
                    Err(async_err) => match $sync::MessagingPre::new(instance_pre) {
                        Ok(p) => Ok(HandlerPre::V0_2(p)),
                        // Report the `@0.3.0` failure too: a guest that meant to
                        // export the async handler but got it subtly wrong would
                        // otherwise only ever be reported against `@0.2.0`.
                        Err(sync_err) => Err(sync_err.context(format!(
                            "component exports neither wasmcloud:messaging/handler@0.3.0 \
                             ({async_err:#}) nor @0.2.0"
                        ))),
                    },
                }
            }

            async fn instantiate(
                &self,
                store: &mut wasmtime::Store<$crate::engine::ctx::SharedCtx>,
            ) -> wasmtime::Result<HandlerProxy> {
                match self {
                    HandlerPre::V0_2(p) => p.instantiate_async(store).await.map(HandlerProxy::V0_2),
                    HandlerPre::V0_3(p) => p.instantiate_async(store).await.map(HandlerProxy::V0_3),
                }
            }
        }

        impl HandlerProxy {
            /// Deliver one message, normalizing the handler's `result` to a
            /// `Result<(), String>` for the ack/log path — `@0.2.0` already
            /// returns a string, `@0.3.0` a `handle-message-error` disposition
            /// rendered by [`render_handle_error`].
            async fn call_handle_message(
                &self,
                store: &mut wasmtime::Store<$crate::engine::ctx::SharedCtx>,
                msg: &types_p2::BrokerMessage,
            ) -> wasmtime::Result<Result<(), String>> {
                match self {
                    HandlerProxy::V0_2(proxy) => {
                        proxy
                            .wasmcloud_messaging0_2_0_handler()
                            .call_handle_message(store, msg)
                            .await
                    }
                    // An `async func` export binds through the concurrent ABI,
                    // so it takes an `Accessor` and must be driven inside
                    // `run_concurrent` — which also lets the guest await its own
                    // imports (e.g. replying via `consumer.publish`) while the
                    // host keeps the store pumping. The `@0.3.0` body is a
                    // native `stream<u8>`, so the delivered bytes are minted
                    // into a stream the guest drains.
                    HandlerProxy::V0_3(proxy) => {
                        let (subject, body, reply_to) =
                            (msg.subject.clone(), msg.body.clone(), msg.reply_to.clone());
                        let outcome = store
                            .run_concurrent(async move |accessor| {
                                let body =
                                    $crate::plugin::wasmcloud_messaging::mint_body(accessor, body)?;
                                let wit_msg =
                                    $async::wasmcloud::messaging0_3_0::types::BrokerMessage {
                                        subject,
                                        body,
                                        reply_to,
                                    };
                                proxy
                                    .wasmcloud_messaging0_3_0_handler()
                                    .call_handle_message(accessor, wit_msg)
                                    .await
                            })
                            .await??;
                        Ok(outcome.map_err(render_handle_error))
                    }
                }
            }
        }

        $crate::plugin::wasmcloud_messaging::messaging_disposition_rendering! {
            disposition: $async::wasmcloud::messaging0_3_0::types::HandleMessageError,
        }
    };
}

pub(crate) use messaging_handler_dispatch;

/// What an unset per-component ceiling resolves to **when pooling is
/// disabled**: how many messages one component may process at once.
///
/// A messaging-triggered component that declared no `poolSize` gets a fresh
/// instance per message, so for those this is equally a ceiling on instances.
/// 32 of a Componentize-Go component (the worst measured shape, at 5 core
/// instances each) is 160 core instances — a bound on one workload's blast
/// radius. A component that *did* declare `poolSize` is bounded by that
/// instead: its deliveries run on warm instances, and only the ones arriving
/// once every instance is busy build a store of their own.
///
/// On a pooled host this constant does not apply: `per_component_ceiling`
/// divides whatever host total is in force, so the number moves with the pool
/// and with `--wasmcloud-messaging-max-in-flight`. The stock pool derives 33.
/// See [`DEFAULT_MAX_IN_FLIGHT_HOST`] for the arithmetic.
pub const DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT: usize = 32;

/// What the host-wide ceiling defaults to **when pooling is disabled**: how
/// many messages *every* messaging component on this host may process at once,
/// added together.
///
/// The per-component ceiling alone does not bound the host — twenty components
/// at 32 is 640 in flight, which at 5 core instances apiece is 3200 against a
/// pool of 1000 ([`crate::engine`] sizes `total_core_instances` from
/// `max_instances`, default 1000). Sizing against the worst measured component
/// shape, 1000 / 5 = 200 is the real ceiling; 128 sits below it and keeps
/// roughly a third of the pool for HTTP-triggered work, warm pools, and
/// long-lived services — the workloads that otherwise fail to *start* when a
/// messaging burst drains the pool.
///
/// With a pool to divide, `derive_host_ceiling` computes that same share from
/// the pool the host actually has rather than from the stock one, so resizing
/// `total_core_instances` resizes the ceiling with it. The stock pool of 1000
/// derives 133 host-wide and 33 per component — close to these constants but
/// not equal to them, because the derivation takes two-thirds of 200 rather
/// than the round number below it. This constant is the fallback for the one
/// case where there is no budget to divide.
///
/// `the_documented_defaults_are_what_the_host_uses` pins both pairs, so the
/// numbers quoted here and in `--wasmcloud-messaging-max-in-flight`'s help
/// cannot drift away from what the host resolves.
///
/// This is a bound, not a reservation: nothing is preallocated, so the default
/// costs nothing on a host that never bursts.
pub const DEFAULT_MAX_IN_FLIGHT_HOST: usize = 128;

/// How long a subscriber loop waits for an admission slot before shedding the
/// message it is holding.
///
/// A bounded wait, not an unbounded one, because parking the loop stops it
/// draining the transport: async-nats `try_send`s into a per-subscription
/// buffer and drops on overflow, so an indefinite park does not preserve the
/// backlog — it converts it into silent loss with no log and no metric. Shedding
/// deliberately at a known deadline turns that into one `warn!` and one counter
/// increment per message, which is the difference between a host an operator can
/// diagnose and one that merely looks idle.
///
/// It also bounds the one inversion this design cannot otherwise avoid: a
/// handler holds its host permit for the whole call, including any outbound
/// `request`, so host-local request/reply between two messaging components can
/// have the responder waiting on a permit the requester is holding. The guest's
/// own `timeout_ms` already breaks that, but only after every such request has
/// burned its full timeout; the deadline here caps how long the queue behind
/// them grows.
///
/// 30s is chosen to be well past any legitimate burst — a message that has
/// waited this long is behind work that is not clearing — and well past typical
/// guest `timeout_ms` values, so the guest's own deadline fires first in the
/// request/reply case and this one only catches genuine saturation.
pub const DEFAULT_ADMISSION_WAIT: Duration = Duration::from_secs(30);

/// The longest [`ADMISSION_WAIT_CONFIG`] may set the wait to.
///
/// Waiting is a real choice — a handler whose work legitimately takes minutes
/// wants its messages queued, not shed — but it is bought with the transport's
/// buffer, and past some point that is a trade nobody would take knowingly. A
/// parked loop is not draining its subscription, async-nats `try_send`s into a
/// per-subscription buffer (65536 messages by default) and drops on overflow,
/// and those drops are counted by nothing: they surface only as a
/// `SlowConsumer` event. So a long wait does not preserve the backlog, it
/// converts *countable* sheds into *uncountable* transport loss. At any rate
/// that saturates a component, the buffer is gone in seconds.
///
/// Ten minutes is far past any handler this is meant to accommodate and far
/// short of "forever". A larger value is clamped to it with a warning rather
/// than rejected, for the same reason an oversized `max_in_flight` is: a
/// manifest written for a different host should still run.
pub const MAX_ADMISSION_WAIT: Duration = Duration::from_secs(600);

/// Worst measured core-instance count for a single component: Componentize-Go,
/// which compiles to `$main`, the `wasi_snapshot_preview1` adapter, and three
/// glue modules. Rust p2 measures 3.
///
/// The pool is spent in *core* instances, so this is what converts "instances
/// the pool can hold" into "messages we may admit".
const WORST_CASE_CORE_INSTANCES_PER_COMPONENT: u32 = 5;

/// Share of the pool's component capacity messaging may claim. The remainder is
/// what HTTP-triggered work, warm pools, and long-lived services draw on — the
/// workloads that otherwise fail to *start* when a messaging burst drains the
/// pool.
const MESSAGING_POOL_SHARE_NUM: u32 = 2;
const MESSAGING_POOL_SHARE_DEN: u32 = 3;

/// Simultaneously-saturated components a host should fit before the host-wide
/// ceiling is what binds. Deriving the per-component default from the host
/// total this way makes that relationship a property of the code rather than a
/// coincidence between two independently-chosen constants — in particular the
/// per-component default can never exceed the host total.
const SATURATED_COMPONENTS_PER_HOST: u32 = 4;

/// Bounds on the derived host ceiling. The floor keeps a host with a tiny pool
/// usable; the cap stops a host with an enormous one producing a number so
/// large it stops being a bound at all. Mirrors
/// `MIN/MAX_DERIVED_MAX_CONNECTIONS` in [`crate::host::quota`].
///
/// The floor is itself bounded by what the pool can hold — see
/// [`derive_host_ceiling`]. A floor that raised the ceiling above the pool's
/// own capacity would admit more work than the pool can instantiate, which is
/// the exact exhaustion these ceilings exist to prevent.
const MIN_DERIVED_IN_FLIGHT: usize = 8;
const MAX_DERIVED_IN_FLIGHT: usize = 4096;

/// Derive the host-wide ceiling from the engine's core-instance budget.
///
/// `total_core_instances` is [`crate::engine::Engine::total_core_instances`]:
/// `None` when pooling is off, in which case there is no budget to divide and
/// the pinned default stands. The §1 failure cannot happen without a pool, but
/// the memory bound still applies, so a ceiling is still wanted.
fn derive_host_ceiling(total_core_instances: Option<u32>) -> usize {
    let Some(total) = total_core_instances else {
        return DEFAULT_MAX_IN_FLIGHT_HOST;
    };
    // Messages the pool could hold if messaging had all of it. `share` is what
    // messaging may actually claim; `capacity` is the hard wall behind it.
    // No overflow: `capacity` is at most `u32::MAX / 5`, so the multiply stays
    // inside `u32`.
    let capacity = total / WORST_CASE_CORE_INSTANCES_PER_COMPONENT;
    let share = capacity * MESSAGING_POOL_SHARE_NUM / MESSAGING_POOL_SHARE_DEN;
    let capacity = usize::try_from(capacity).unwrap_or(MAX_DERIVED_IN_FLIGHT);
    let share = usize::try_from(share).unwrap_or(MAX_DERIVED_IN_FLIGHT);
    // The floor may not raise the ceiling past what the pool actually holds.
    // On a small pool the honest answer is a small number: a host that cannot
    // run more than two messages at once should say two and shed the rest,
    // not admit eight and fail at instantiation. `max(1)` because a pool too
    // small for even one component still has to name some ceiling, and one is
    // the least wrong of the available answers.
    let floor = MIN_DERIVED_IN_FLIGHT.min(capacity).max(1);
    share.clamp(floor, MAX_DERIVED_IN_FLIGHT)
}

/// The per-component ceiling implied by a host-wide one.
///
/// Always derived from the host total that is actually in force — including an
/// operator-supplied one — so the two knobs compose: raising the host ceiling
/// raises what a single component may use, and lowering it lowers it. Deriving
/// this from the *pool* instead would leave `--wasmcloud-messaging-max-in-flight`
/// unable to raise any individual component past a number the operator never
/// chose, and could place the per-component default above an explicitly-set
/// host total.
///
/// `max(1)` keeps a host whose total is below
/// [`SATURATED_COMPONENTS_PER_HOST`] able to run one message at a time rather
/// than none. The result is never above `host_total`, so the two can never
/// contradict.
fn per_component_ceiling(host_total: usize) -> usize {
    (host_total / SATURATED_COMPONENTS_PER_HOST as usize).max(1)
}

/// The two-level admission ceiling on messaging-triggered work, built once per
/// host and shared by every messaging backend on it.
///
/// Both levels are needed and neither subsumes the other: the per-component
/// ceiling stops one runaway workload taking the host, and the host-wide
/// ceiling stops a crowd of components each sitting inside its own limit from
/// adding up to more than the pool can carry.
#[derive(Clone, Debug)]
pub struct MessagingLimits {
    /// Shared across every component on this host. Cloned into each
    /// subscriber loop's [`Admission`].
    host: Arc<Semaphore>,
    host_total: usize,
    per_component_default: usize,
    admission_wait: Duration,
    timeouts: AdmissionTimeouts,
    /// Per-component gates, keyed by manifest identity so replicas of one
    /// deployment share one. See [`ComponentGates`].
    gates: ComponentGates,
}

/// The per-component gates on this host, keyed by [`AdmissionIdentity`].
///
/// **Keyed by manifest identity, not by component id.** Every replica of a
/// deployment is a separate workload with its own `uuid::Uuid::new_v4()`
/// component id, so keying by that gives each replica its own full ceiling:
/// four replicas of a component at `max_in_flight: 32` could hold 128 messages
/// on one host, against a stock host-wide total of 133. The per-component
/// ceiling exists to stop one workload taking the host, and per-replica gates
/// let it do the opposite. Keyed by identity, `max_in_flight` means what a
/// manifest author reads it to mean: a total for that component, however many
/// replicas of it this host happens to run.
///
/// The namespace is part of the key because workload names are only unique
/// within one — without it, two teams' `ingester` workloads would contend for
/// a single gate.
///
/// An entry is reference-counted by *binding*, not by [`Admission`] clone:
/// [`MessagingLimits::admission`] takes a reference and [`Admission::close`]
/// gives it back, so the gate closes when the last replica on this host goes
/// away and not before. `close` takes `self` by value, which is what the
/// backends' `component_cleanup` has, so one binding cannot return its
/// reference twice.
#[derive(Clone, Debug, Default)]
struct ComponentGates {
    // `std::sync::Mutex`: every critical section is a map lookup with no await
    // inside, so an async mutex would buy nothing and cost a scheduling point
    // on the message path.
    inner: Arc<std::sync::Mutex<std::collections::HashMap<AdmissionIdentity, GateEntry>>>,
}

#[derive(Debug)]
struct GateEntry {
    semaphore: Arc<Semaphore>,
    /// The ceiling the first binding resolved. Kept to notice a later binding
    /// asking for a different one, which means two genuinely different
    /// components collided on one identity.
    limit: usize,
    /// Live bindings sharing this gate — replicas of one deployment on this
    /// host.
    bindings: usize,
}

impl ComponentGates {
    /// Take a reference to `identity`'s gate, creating it at `limit` if this is
    /// the first binding.
    ///
    /// Returns the shared semaphore and the ceiling actually in force, which is
    /// the first binding's where they disagree — a semaphore cannot be resized
    /// under tasks already holding permits, and silently adopting the newer
    /// number would change a running component's ceiling.
    fn acquire(&self, identity: &AdmissionIdentity, limit: usize) -> (Arc<Semaphore>, usize) {
        let mut gates = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = gates.entry(identity.clone()).or_insert_with(|| GateEntry {
            semaphore: Arc::new(Semaphore::new(limit)),
            limit,
            bindings: 0,
        });
        entry.bindings += 1;
        if entry.limit != limit {
            tracing::warn!(
                namespace = %identity.namespace,
                workload = %identity.workload,
                component = %identity.component,
                in_force = entry.limit,
                requested = limit,
                "two messaging components share one workload/component name but ask for \
                 different max_in_flight ceilings; keeping the first. Replicas of one \
                 deployment share a gate, so this means two distinct components collided \
                 on one name"
            );
        }
        (Arc::clone(&entry.semaphore), entry.limit)
    }

    /// Give back one binding's reference, closing and dropping the gate once
    /// the last replica is gone.
    ///
    /// Closing wakes any loop parked on a saturated gate with
    /// [`Admitted::Closed`]. Closing while another replica is still running
    /// would stop *its* subscriber loop, which is why this is refcounted rather
    /// than closing on the first teardown.
    fn release(&self, identity: &AdmissionIdentity) {
        let mut gates = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = gates.get_mut(identity) else {
            return;
        };
        entry.bindings = entry.bindings.saturating_sub(1);
        if entry.bindings == 0 {
            entry.semaphore.close();
            gates.remove(identity);
        }
    }

    #[cfg(test)]
    fn bindings(&self, identity: &AdmissionIdentity) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(identity)
            .map_or(0, |e| e.bindings)
    }
}

impl Default for MessagingLimits {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_IN_FLIGHT_HOST,
            DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT,
        )
    }
}

impl MessagingLimits {
    /// Build the host's messaging ceilings, resolving whatever the operator did
    /// not specify against the engine's core-instance budget.
    ///
    /// `None` for either ceiling means "the operator said nothing" — which is
    /// why the flags must not carry a `default_value_t`. A CLI-parse-time
    /// default is indistinguishable downstream from an operator typing the same
    /// number, and the information needed to do better is gone by the time this
    /// runs.
    ///
    /// The host total resolves first and the per-component default is derived
    /// from **that**, not from the pool, so setting one knob moves the other
    /// with it. Deriving both independently would make
    /// `--wasmcloud-messaging-max-in-flight 1024` a no-op for every individual
    /// component, and `--wasmcloud-messaging-max-in-flight 4` produce a
    /// per-component default above the host total the operator just set.
    pub fn resolve(
        host_total: Option<usize>,
        per_component_default: Option<usize>,
        total_core_instances: Option<u32>,
    ) -> Self {
        let host_total = host_total.unwrap_or_else(|| derive_host_ceiling(total_core_instances));
        let per_component_default =
            per_component_default.unwrap_or_else(|| per_component_ceiling(host_total));
        Self::new(host_total, per_component_default)
    }

    /// The largest ceiling either knob may take: what a [`Semaphore`] can hold.
    ///
    /// Above this `Semaphore::new` panics, so a number beyond it is not a large
    /// limit but a crash at host startup. The CLI rejects it with a config
    /// error (see `wash`'s config layer); [`MessagingLimits::new`] clamps, for
    /// the same belt-and-braces reason it floors a zero.
    pub const MAX_IN_FLIGHT: usize = Semaphore::MAX_PERMITS;

    /// Build the host's messaging ceilings.
    ///
    /// Both arguments are clamped into `1..=`[`Self::MAX_IN_FLIGHT`]: a zero
    /// would mean "process no messages", which is never what an operator meant,
    /// and anything above the maximum would panic inside [`Semaphore::new`]
    /// rather than act as a ceiling. The CLI rejects both outright (see `wash`'s
    /// config layer) so this is a belt-and-braces bound for programmatic
    /// callers.
    pub fn new(host_total: usize, per_component_default: usize) -> Self {
        let host_total = host_total.clamp(1, Self::MAX_IN_FLIGHT);
        let per_component_default = per_component_default.clamp(1, Self::MAX_IN_FLIGHT);
        if per_component_default > host_total {
            // Harmless — the host semaphore gates first regardless — but
            // almost certainly an operator mixing the two knobs up. Mirrors
            // the warning `connection_quotas` emits for the same shape.
            tracing::warn!(
                per_component_default,
                host_total,
                "the per-component messaging ceiling exceeds the host-wide total; \
                 the host-wide cap will gate first"
            );
        }
        Self {
            host: Arc::new(Semaphore::new(host_total)),
            host_total,
            per_component_default,
            admission_wait: DEFAULT_ADMISSION_WAIT,
            timeouts: AdmissionTimeouts::new(),
            gates: ComponentGates::default(),
        }
    }

    /// Override how long a subscriber loop waits for a slot before shedding the
    /// message it is holding. See [`DEFAULT_ADMISSION_WAIT`].
    ///
    /// Held to [`MAX_ADMISSION_WAIT`], the same bound the per-component config
    /// key takes. Clamping only at the config layer would leave this — the way
    /// an embedder sets the host-wide default — able to reinstate the unbounded
    /// park that bound exists to prevent, and would make the maximum a property
    /// of one code path rather than of the type.
    #[must_use]
    pub fn with_admission_wait(mut self, wait: Duration) -> Self {
        self.admission_wait = clamp_admission_wait(wait);
        self
    }

    /// How long a subscriber loop waits for a slot before shedding.
    pub fn admission_wait(&self) -> Duration {
        self.admission_wait
    }

    /// The host-wide ceiling.
    pub fn host_total(&self) -> usize {
        self.host_total
    }

    /// What an unset component field resolves to.
    pub fn per_component_default(&self) -> usize {
        self.per_component_default
    }

    /// Resolve one component's configured ceiling into its admission gate.
    ///
    /// `requested` is what [`parse_max_in_flight`] made of the component's
    /// [`MAX_IN_FLIGHT_CONFIG`] entry: `None` where the component named no
    /// ceiling of its own.
    ///
    /// An explicit request is clamped to **both** ceilings: the per-component
    /// one, which the operator flag documents as "the most any single component
    /// may ask for", and the host-wide total. Clamping to the host total alone
    /// would leave the per-component flag enforcing nothing — a workload
    /// manifest could name the whole host budget and starve every co-tenant
    /// component on the host, which is precisely the runaway the per-component
    /// ceiling exists to stop.
    ///
    /// Clamped rather than rejected, because a manifest that outlives the host
    /// it was written for should still run: the host it lands on may be smaller
    /// than the one it was sized against, and refusing to start is a worse
    /// answer than running at the ceiling that host can actually offer.
    /// `identity` is the manifest identity of what is being bound. It selects
    /// the gate as well as labelling it: every replica of one deployment on
    /// this host resolves to the same identity and therefore shares one
    /// ceiling. See [`ComponentGates`].
    pub(crate) fn admission(
        &self,
        identity: &AdmissionIdentity,
        requested: Option<usize>,
    ) -> Admission {
        let ceiling = self.per_component_default.min(self.host_total);
        let resolved = match requested {
            Some(requested) if requested > ceiling => {
                tracing::warn!(
                    requested,
                    ceiling,
                    per_component_default = self.per_component_default,
                    host_total = self.host_total,
                    "component asked for a higher messaging max_in_flight than this host allows; \
                     clamping to the host's per-component ceiling"
                );
                ceiling
            }
            Some(requested) => requested,
            // Unset: the default is already within both ceilings by
            // construction, but `min` keeps that true if either is reconfigured.
            None => ceiling,
        }
        // A zero would build a gate that admits nothing, ever — a component
        // that silently processes no messages. `parse_max_in_flight` already
        // spells zero as "unset", so this only catches a programmatic caller,
        // and it floors for the same reason `MessagingLimits::new` does.
        .max(1);
        let (component, limit) = self.gates.acquire(identity, resolved);
        Admission {
            component,
            host: Arc::clone(&self.host),
            limit,
            wait: self.admission_wait,
            timeouts: self.timeouts.clone(),
            subscriptions: Arc::from([]),
            identity: identity.clone(),
            gates: self.gates.clone(),
        }
    }
}

/// Counters for messages the admission gate could not admit in time. Built once
/// per host and cloned into every [`Admission`], so a single
/// `messaging.admission.shed` series covers the host, split by the manifest
/// identity of whatever is shedding.
#[derive(Clone, Debug)]
struct AdmissionTimeouts {
    shed: Counter<u64>,
}

/// Attribute value used when a shed message matches none of the component's
/// configured patterns — which should not happen, since the message arrived on
/// one of them, but a metric attribute must never be derived from traffic.
const UNMATCHED_SUBSCRIPTION: &str = "<unmatched>";

/// Who is shedding, in terms a manifest author and a dashboard both recognize.
///
/// Deliberately **not** the component id: that is a `uuid::Uuid::new_v4()`
/// minted per workload construction (`engine::workload`), so attributing a
/// counter with it mints a fresh time series on every restart, rolling update,
/// and replica — unbounded growth driven by deployment churn, and a value no
/// operator can map back to a workload. Every field here comes from the
/// manifest instead, so the series count is bounded by what is deployed rather
/// than by how often it is redeployed.
///
/// Named to match the span convention the host already uses for workload
/// identity (`workload.id` / `workload.name` / `workload.namespace`, see
/// `host::HostApi::start_workload`). The component id keeps its place on the
/// shed `warn!`, where identity is per-event and therefore free, and where it
/// still joins to the `wasmcloud.messaging.on_workload_resolved` span.
/// It is also the key the per-component gate is registered under, so that
/// replicas of one deployment share one ceiling — see [`ComponentGates`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct AdmissionIdentity {
    namespace: Arc<str>,
    workload: Arc<str>,
    component: Arc<str>,
}

impl AdmissionIdentity {
    /// Build the identity from a bound item's manifest names. `WorkloadItem`
    /// derefs to the workload metadata, so both backends have all three in hand
    /// at bind time.
    pub(crate) fn new(namespace: &str, workload: &str, component: &str) -> Self {
        Self {
            namespace: Arc::from(namespace),
            workload: Arc::from(workload),
            component: Arc::from(component),
        }
    }
}

impl AdmissionTimeouts {
    fn new() -> Self {
        Self {
            shed: opentelemetry::global::meter("wash-runtime")
                .u64_counter("messaging.admission.shed")
                .with_description(
                    "Messages dropped because no admission slot became free before the deadline",
                )
                .build(),
        }
    }

    /// Record one shed message.
    ///
    /// Every attribute is bounded by configuration rather than by traffic or by
    /// deployment churn — see [`AdmissionIdentity`] for why the component id is
    /// not among them.
    ///
    /// `subscription` is the *configured pattern* the message arrived on, never
    /// the concrete subject. A component has as many patterns as its
    /// `subscriptions` names. Attributing the concrete subject instead would
    /// mint a new series per distinct subject under a wildcard subscription —
    /// unbounded growth, arriving precisely during the flood this counter exists
    /// to report, when the metrics pipeline can least afford it. The concrete
    /// subject is on the `warn!` at the call site, where it costs nothing.
    fn record(&self, identity: &AdmissionIdentity, subscription: &str) {
        self.shed.add(
            1,
            &[
                KeyValue::new("workload.namespace", identity.namespace.to_string()),
                KeyValue::new("workload.name", identity.workload.to_string()),
                // The component's *manifest* name. Kept alongside the workload
                // so a workload running more than one messaging component can
                // still be told apart; bounded by the manifest exactly as the
                // two above are.
                KeyValue::new("component", identity.component.to_string()),
                KeyValue::new("subscription", subscription.to_string()),
            ],
        );
    }
}

/// Detail carried on the [`MsgError::QuotaExceeded`] a shed request turns into.
///
/// [`MsgError::QuotaExceeded`] rather than a new case because that is precisely
/// what happened — a rate limit was exceeded — and it already exists on both
/// messaging surfaces, so the async WIT lowers it to `quota-exceeded` with no
/// new vocabulary.
pub(crate) const ADMISSION_SHED_DETAIL: &str = "the responding component's messaging admission gate is saturated; \
     the request was shed rather than queued";

/// The error a requester gets when the host shed its request.
///
/// **Only the in-memory backend produces this.** Telling a requester means
/// answering on its reply subject, and a `request` resolves on the *first*
/// message to reach its inbox — so where several components subscribe to one
/// subject, a saturated component's instant notice beats a healthy one's real
/// reply and fails a request that was about to succeed. The in-memory backend
/// routes its own fan-out and so knows when a component is the sole subscriber
/// ([`in_memory`]); a NATS subscriber cannot know that, and there the caller's
/// `timeout_ms` remains the only honest signal.
pub(crate) fn shed_error() -> MsgError {
    MsgError::QuotaExceeded(ADMISSION_SHED_DETAIL.to_string())
}

/// What [`Admission::acquire_before_deadline`] settled on.
#[derive(Debug)]
pub(crate) enum Admitted {
    /// A slot was free, or came free inside the deadline.
    Slot(AdmissionPermit),
    /// Nothing came free in time; the caller must shed the message it holds.
    Shed,
    /// A semaphore was closed: the component is going away and the loop should
    /// stop.
    Closed,
}

/// One component's admission gate: its own ceiling plus the shared host one.
#[derive(Clone, Debug)]
pub(crate) struct Admission {
    component: Arc<Semaphore>,
    host: Arc<Semaphore>,
    limit: usize,
    wait: Duration,
    timeouts: AdmissionTimeouts,
    /// The component's configured subscription patterns, used to attribute a
    /// shed message to a pattern rather than to its concrete subject. Set by
    /// [`Admission::with_subscriptions`] once the backend has parsed them;
    /// empty until then, and empty is also legitimate (it means "everything").
    subscriptions: Arc<[String]>,
    /// Manifest identity of whatever this gate fronts. Both the attribution on
    /// a shed message and the key the gate is registered under.
    identity: AdmissionIdentity,
    /// The host's gate registry, so teardown can give this binding's reference
    /// back. Held rather than reached through `MessagingLimits` because an
    /// `Admission` outlives the borrow it was built from.
    gates: ComponentGates,
}

impl Admission {
    /// Take one slot, parking until both levels have room.
    ///
    /// Returns `None` once either semaphore is closed, which is how a
    /// shutting-down component tells its subscriber loop to stop.
    ///
    /// **Order matters: component first, then host.** A task holding a *host*
    /// permit while it waited for a component permit would block every other
    /// component on the host; holding a component permit while waiting for the
    /// host's only throttles the component that is already at its own limit.
    /// There is no deadlock either way — two semaphores, one consistent order,
    /// no cycle — but only this order confines the head-of-line blocking to
    /// the workload responsible for it.
    ///
    /// Cancel-safe: dropping the returned future before it resolves releases
    /// whichever permit it had already taken.
    pub(crate) async fn acquire(&self) -> Option<AdmissionPermit> {
        let component = Arc::clone(&self.component).acquire_owned().await.ok()?;
        let host = Arc::clone(&self.host).acquire_owned().await.ok()?;
        Some(AdmissionPermit {
            _component: component,
            _host: host,
        })
    }

    /// [`Admission::acquire`], but giving up after [`MessagingLimits::admission_wait`]
    /// so the caller can shed the message and get back to draining the
    /// transport rather than parking on a saturated host indefinitely.
    ///
    /// Logging and the counter live here rather than at each call site so both
    /// backends shed identically and a new backend cannot forget to.
    ///
    /// Cancel-safe for the same reason [`Admission::acquire`] is: the timeout
    /// wraps the acquire future whole, so expiring drops it and releases
    /// whichever permit it had already taken.
    pub(crate) async fn acquire_before_deadline(
        &self,
        component_id: &str,
        subject: &str,
    ) -> Admitted {
        match tokio::time::timeout(self.wait, self.acquire()).await {
            Ok(Some(permit)) => Admitted::Slot(permit),
            Ok(None) => Admitted::Closed,
            Err(_) => {
                self.timeouts
                    .record(&self.identity, self.subscription_for(subject));
                // The component id stays here rather than on the counter:
                // per-event identity is free, and it joins this line to the
                // `wasmcloud.messaging.on_workload_resolved` span. The manifest
                // names come along so the warning names the same thing the
                // metric does.
                tracing::warn!(
                    %component_id,
                    workload.namespace = %self.identity.namespace,
                    workload.name = %self.identity.workload,
                    component = %self.identity.component,
                    %subject,
                    waited = ?self.wait,
                    limit = self.limit,
                    "no messaging admission slot came free before the deadline; \
                     dropping message. The host or this component is saturated — \
                     raise --wasmcloud-messaging-max-in-flight, raise the component's \
                     max_in_flight config, or reduce inbound rate"
                );
                Admitted::Shed
            }
        }
    }

    /// The resolved per-component ceiling, after defaulting and clamping.
    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    /// Give this binding's reference to the component gate back on teardown.
    ///
    /// The gate is closed — waking any loop parked in
    /// [`Admission::acquire_before_deadline`] with [`Admitted::Closed`] — only
    /// once the *last* binding releases it. Replicas of one deployment share a
    /// gate (see [`ComponentGates`]), so closing on the first teardown would
    /// stop a healthy replica's subscriber loop dead: it would see `Closed`,
    /// break, and silently never receive another message. Refcounting is what
    /// makes gate sharing safe to combine with the round-2 close-on-teardown
    /// behavior instead of having to choose between them.
    ///
    /// Only the component gate is ever closed. The host semaphore is shared by
    /// every messaging component on the host and must outlive all of them.
    ///
    /// Takes `self` by value — which is what the backends' `component_cleanup`
    /// has, since it owns the `ComponentData` it is handed — so one binding
    /// cannot return its reference twice and drive the count to zero under a
    /// live replica. Clones handed to subscriber loops are not bindings and
    /// never release.
    ///
    /// The per-component cancel token also wakes that loop, and both backends
    /// select on it, so this remains belt-and-braces for the last replica; what
    /// it adds is that the documented `Closed` signal actually fires.
    pub(crate) fn close(self) {
        self.gates.release(&self.identity);
    }

    /// Override how long this component's loop waits before shedding, from its
    /// [`ADMISSION_WAIT_CONFIG`] entry. `None` leaves the host default in
    /// place.
    ///
    /// Clamped to [`MAX_ADMISSION_WAIT`] like every other way of setting a
    /// wait. Redundant for the config path, which arrives already clamped from
    /// [`parse_admission_wait`], and deliberately so: the bound holds because
    /// no setter lets it through, not because each caller remembered.
    #[must_use]
    pub(crate) fn with_admission_wait(mut self, wait: Option<Duration>) -> Self {
        if let Some(wait) = wait {
            self.wait = clamp_admission_wait(wait);
        }
        self
    }

    /// How long this gate waits before shedding.
    #[cfg(test)]
    pub(crate) fn wait(&self) -> Duration {
        self.wait
    }

    /// Attach the component's configured subscription patterns, so a shed
    /// message can be attributed to the pattern it arrived on.
    ///
    /// Separate from [`MessagingLimits::admission`] because the ceiling and the
    /// subscriptions are read from different config keys and the backends parse
    /// them at slightly different points; a builder keeps `admission` from
    /// growing a second argument every backend has to remember to thread.
    #[must_use]
    pub(crate) fn with_subscriptions(mut self, subscriptions: &[String]) -> Self {
        self.subscriptions = subscriptions.into();
        self
    }

    /// The configured pattern `subject` arrived on.
    ///
    /// An empty pattern list means "everything" (single-handler back-compat),
    /// which is itself one bounded value. Anything unmatched falls back to a
    /// constant rather than to the subject, so this can never return a value
    /// derived from traffic.
    fn subscription_for(&self, subject: &str) -> &str {
        if self.subscriptions.is_empty() {
            return ">";
        }
        self.subscriptions
            .iter()
            .find(|pattern| subject_matches(pattern, subject))
            .map_or(UNMATCHED_SUBSCRIPTION, String::as_str)
    }
}

/// Both permits for one in-flight message, released together on drop.
///
/// Held by the spawned handler task, so a slot is freed when the handler
/// returns — whether it succeeded, failed, or trapped — and equally on the
/// error paths between admission and spawn, where this is simply a local that
/// falls out of scope.
#[derive(Debug)]
pub(crate) struct AdmissionPermit {
    _component: OwnedSemaphorePermit,
    _host: OwnedSemaphorePermit,
}

/// Returns `true` if the world exports the `wasmcloud:messaging/handler`
/// interface at any version. Matches via [`WitInterface::contains`] rather
/// than set equality, so an exported `handler@0.2.x` is recognized no matter
/// which exact version the component was built against.
pub(crate) fn exports_messaging_handler(world: &crate::wit::WitWorld) -> bool {
    let handler = crate::wit::WitInterface::from("wasmcloud:messaging/handler");
    world.exports.iter().any(|e| e.contains(&handler))
}

/// Returns `true` if the workload declared its `wasmcloud:messaging` host
/// interface at the async revision (`>= 0.3.0`).
///
/// A versionless declaration is deliberately *not* async: it cannot be told
/// apart from a `@0.2.0` one, and binding the wrong ABI fails at instantiation
/// with an opaque type mismatch. Declaring the version is how a workload selects
/// a surface, and defaulting to sync keeps pre-`@0.3.0` workloads working.
pub(crate) fn declares_async_messaging(interfaces: &crate::plugin::WitInterfaces<'_>) -> bool {
    const ASYNC_MIN: semver::Version = semver::Version::new(0, 3, 0);
    interfaces.iter().any(|i| {
        i.namespace == "wasmcloud"
            && i.package == "messaging"
            && i.version.as_ref().is_some_and(|v| *v >= ASYNC_MIN)
    })
}

/// Config key naming a component's messaging admission ceiling.
///
/// Read from the component's `LocalResources.config`, falling back to the
/// workload-scoped `wasmcloud:messaging` host interface config — the same two
/// places, in the same order, as [`parse_subscriptions`]'s `subscriptions` and
/// the NATS backend's `consumer_group`. This is plugin configuration and only
/// the messaging plugin reads it, so it travels the way the plugin's other
/// per-component settings do rather than as a field on a core engine type.
///
/// The ceiling covers the component across every replica of it on this host —
/// see [`ComponentGates`] — so scaling a deployment out does not multiply it.
/// It can only lower a component below the host's per-component default, never
/// raise it above: [`MessagingLimits::admission`] clamps to both ceilings.
pub(crate) const MAX_IN_FLIGHT_CONFIG: &str = "max_in_flight";

/// Config key naming how long this component's subscriber loop waits for an
/// admission slot before shedding.
///
/// Read from the same two places as [`MAX_IN_FLIGHT_CONFIG`], and per component
/// rather than per host because the right answer depends on the handler: a
/// component whose work legitimately takes minutes wants to wait, and one
/// serving interactive traffic wants to shed early and stay responsive. A
/// single host-wide number cannot be right for both, and
/// [`DEFAULT_ADMISSION_WAIT`] alone would turn a slow handler's queued messages
/// into dropped ones with no way to say otherwise.
///
/// **What raising it costs.** A loop waiting for a slot is not draining its
/// subscription, and the transport's buffer is what absorbs the difference —
/// silently, since an overflow there is dropped without a counter. Raising this
/// therefore trades sheds you can see for losses you cannot; it is the right
/// trade for a handler that genuinely needs minutes and the wrong one as a
/// reflex against shed warnings, where the answer is `max_in_flight` or fewer
/// messages. Bounded by [`MAX_ADMISSION_WAIT`] so it cannot become "forever".
pub(crate) const ADMISSION_WAIT_CONFIG: &str = "admission_wait";

/// Parses an [`ADMISSION_WAIT_CONFIG`] value into a wait duration.
///
/// Accepts what `humantime` accepts — `30s`, `5m`, `1m30s`, `500ms` — and a
/// bare integer as seconds, since that is what an operator used to plain
/// numeric knobs is most likely to write. `None` for absent, empty, or
/// unparseable; the caller falls back to [`DEFAULT_ADMISSION_WAIT`].
///
/// A zero wait is honored rather than treated as unset: "shed immediately if no
/// slot is free" is a coherent policy for latency-sensitive work, and unlike a
/// zero *ceiling* it does not mean "process nothing" — it means "do not queue".
///
/// Bounded above by [`MAX_ADMISSION_WAIT`]. Raising this queues messages that
/// would otherwise be shed, which is the point — but it buys that with the
/// transport's buffer, and past the cap the trade stops being one anybody would
/// make on purpose. See [`MAX_ADMISSION_WAIT`].
pub(crate) fn parse_admission_wait(raw: Option<&str>) -> Option<Duration> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty())?;
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(clamp_admission_wait(Duration::from_secs(secs)));
    }
    match humantime::parse_duration(raw) {
        Ok(d) => Some(clamp_admission_wait(d)),
        Err(_) => {
            tracing::warn!(
                config_key = ADMISSION_WAIT_CONFIG,
                value = raw,
                default = ?DEFAULT_ADMISSION_WAIT,
                "messaging admission_wait is not a duration (expected e.g. `45s`, `2m`, \
                 or a bare number of seconds); falling back to the default"
            );
            None
        }
    }
}

/// Hold a configured wait to [`MAX_ADMISSION_WAIT`], warning when it bites so
/// the operator learns the number in force is not the one they wrote.
fn clamp_admission_wait(wait: Duration) -> Duration {
    if wait > MAX_ADMISSION_WAIT {
        tracing::warn!(
            config_key = ADMISSION_WAIT_CONFIG,
            requested = ?wait,
            max = ?MAX_ADMISSION_WAIT,
            "messaging admission_wait exceeds the maximum; clamping. Waiting longer does not \
             preserve the backlog — a parked subscriber stops draining its subscription, and \
             the transport drops on buffer overflow without counting it"
        );
        return MAX_ADMISSION_WAIT;
    }
    wait
}

/// Parses a [`MAX_IN_FLIGHT_CONFIG`] value into an explicit ceiling.
///
/// `None` — absent, empty, unparseable, or zero — means "unset", which
/// [`MessagingLimits::admission`] resolves to the host's per-component default.
/// A malformed value warns rather than failing the component: a typo in one
/// tuning knob should not stop a workload from starting, and the default it
/// falls back to is the safe direction.
pub(crate) fn parse_max_in_flight(raw: Option<&str>) -> Option<usize> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty())?;
    match raw.parse::<usize>() {
        // Zero spells "unset" here for the same reason it does on the pool
        // limits: it can only have meant "no ceiling of my own", never
        // "process no messages".
        Ok(0) => None,
        Ok(v) => Some(v),
        Err(_) => {
            tracing::warn!(
                config_key = MAX_IN_FLIGHT_CONFIG,
                value = raw,
                "messaging max_in_flight is not a non-negative integer; \
                 falling back to the host's per-component default"
            );
            None
        }
    }
}

/// Returns whether `subject` matches NATS subscription `pattern`, where `*`
/// matches exactly one token and `>` matches one or more trailing tokens.
pub(crate) fn subject_matches(pattern: &str, subject: &str) -> bool {
    let mut subject_tokens = subject.split('.');
    let mut pattern_tokens = pattern.split('.').peekable();
    while let Some(pat) = pattern_tokens.next() {
        if pat == ">" {
            // `>` is only valid as the final token and matches one or more
            // remaining subject tokens.
            return pattern_tokens.peek().is_none() && subject_tokens.next().is_some();
        }
        match subject_tokens.next() {
            Some(sub) if pat == "*" || pat == sub => continue,
            _ => return false,
        }
    }
    // Every pattern token matched; the subject must be fully consumed too.
    subject_tokens.next().is_none()
}

/// Parses a comma-separated `subscriptions` config value into trimmed,
/// non-empty subjects. Shared by the in-memory and NATS backends so they
/// agree on how a configured subscription string maps to subjects.
pub(crate) fn parse_subscriptions(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        MsgError, declares_async_messaging, exports_messaging_handler, parse_subscriptions,
    };
    use crate::plugin::WitInterfaces;
    use crate::wit::{WitInterface, WitWorld};
    use std::collections::HashSet;

    /// A fresh identity per call, so a test that wants independent per-component
    /// gates gets them. Gates are keyed by identity now (replicas of one
    /// deployment share one), so a test asserting that two components do *not*
    /// contend has to name them differently — which is exactly what production
    /// does. Tests about sharing build their identity explicitly instead.
    fn unique_identity() -> super::AdmissionIdentity {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        super::AdmissionIdentity::new("test-ns", "test-workload", &format!("component-{n}"))
    }

    fn messaging_iface(version: Option<&str>) -> WitInterface {
        WitInterface {
            namespace: "wasmcloud".to_string(),
            package: "messaging".to_string(),
            interfaces: ["consumer".to_string()].into_iter().collect(),
            version: version.map(|v| semver::Version::parse(v).unwrap()),
            config: std::collections::HashMap::new(),
            name: None,
        }
    }

    /// Which surface a workload gets is decided by the version it declares.
    #[test]
    fn async_surface_selected_by_declared_version() {
        for (version, expected) in [
            (Some("0.3.0"), true),
            (Some("0.4.0"), true),
            (Some("0.2.0"), false),
        ] {
            let set = HashSet::from([messaging_iface(version)]);
            assert_eq!(
                declares_async_messaging(&WitInterfaces::new(&set)),
                expected,
                "version {version:?}"
            );
        }
    }

    /// A versionless declaration must resolve to the SYNC surface: it is
    /// indistinguishable from a `@0.2.0` one, and defaulting the other way would
    /// silently break every workload written before `@0.3.0` existed.
    #[test]
    fn versionless_declaration_defaults_to_sync() {
        let set = HashSet::from([messaging_iface(None)]);
        assert!(!declares_async_messaging(&WitInterfaces::new(&set)));
    }

    /// An unrelated package must never flip the messaging surface.
    #[test]
    fn other_packages_do_not_select_the_async_surface() {
        let mut iface = messaging_iface(Some("0.3.0"));
        iface.package = "keyvalue".to_string();
        let set = HashSet::from([iface]);
        assert!(!declares_async_messaging(&WitInterfaces::new(&set)));
    }

    /// The sync `@0.2.0` surface prints the classified error's detail verbatim,
    /// so its error strings are unchanged from before `MsgError` existed.
    #[test]
    fn sync_lowering_preserves_error_text() {
        let text: String = MsgError::Timeout("request timed out after 5000ms".into()).into();
        assert_eq!(text, "request timed out after 5000ms");
        let text: String =
            MsgError::BrokerUnavailable("failed to send message: broken pipe".into()).into();
        assert_eq!(text, "failed to send message: broken pipe");
    }

    #[test]
    fn recognizes_exported_handler_at_any_version() {
        for export in [
            "wasmcloud:messaging/handler",
            "wasmcloud:messaging/handler@0.2.0",
            "wasmcloud:messaging/handler@0.2.2",
        ] {
            let world = WitWorld {
                imports: HashSet::new(),
                exports: HashSet::from([WitInterface::from(export)]),
            };
            assert!(exports_messaging_handler(&world), "should match {export}");
        }
    }

    #[test]
    fn ignores_non_handler_worlds() {
        // Importing the handler is not exporting it
        let importer = WitWorld {
            imports: HashSet::from([WitInterface::from("wasmcloud:messaging/handler@0.2.0")]),
            exports: HashSet::new(),
        };
        assert!(!exports_messaging_handler(&importer));

        // Exporting other messaging interfaces does not count
        let consumer = WitWorld {
            imports: HashSet::new(),
            exports: HashSet::from([WitInterface::from("wasmcloud:messaging/consumer,types")]),
        };
        assert!(!exports_messaging_handler(&consumer));
    }

    #[test]
    fn parses_single_subject() {
        assert_eq!(
            parse_subscriptions(Some("tasks.task-worker")),
            vec!["tasks.task-worker".to_string()]
        );
    }

    #[test]
    fn parses_multiple_subjects() {
        assert_eq!(
            parse_subscriptions(Some("a,b,c")),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn trims_surrounding_whitespace_and_drops_empties() {
        assert_eq!(
            parse_subscriptions(Some(" tasks.leet , tasks.reverse ,, ")),
            vec!["tasks.leet".to_string(), "tasks.reverse".to_string()]
        );
        assert!(parse_subscriptions(None).is_empty());
    }

    // --- Admission ceilings -------------------------------------------------

    use super::{
        Admitted, DEFAULT_MAX_IN_FLIGHT_HOST, DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT, MessagingLimits,
    };
    use futures::FutureExt as _;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn unset_resolves_to_the_per_component_default() {
        let limits = MessagingLimits::default();
        assert_eq!(
            limits.admission(&unique_identity(), None).limit(),
            DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT,
        );
    }

    #[test]
    fn every_spelling_of_unset_parses_to_none() {
        // The config value is a string now, so "unset" has more spellings than
        // the wire's non-positive integer had: absent, empty, whitespace, zero,
        // and anything that is not a number at all. All must reach the host
        // default rather than a ceiling of their own.
        for unset in [None, Some(""), Some("   "), Some("0"), Some("-1")] {
            assert_eq!(
                super::parse_max_in_flight(unset),
                None,
                "{unset:?} should parse as unset"
            );
        }
        // A typo must not take the workload down with it.
        assert_eq!(super::parse_max_in_flight(Some("thirty-two")), None);
        assert_eq!(super::parse_max_in_flight(Some("32.5")), None);
    }

    #[test]
    fn the_two_knobs_compose() {
        // Setting one knob must move the other with it. Deriving both from the
        // pool independently made `--wasmcloud-messaging-max-in-flight` a no-op
        // for any individual component: the host ceiling rose and every
        // component stayed pinned at the pool-derived per-component default.
        let raised = MessagingLimits::resolve(Some(1024), None, Some(1000));
        assert_eq!(raised.host_total(), 1024);
        assert!(
            raised.per_component_default() > DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT,
            "raising the host ceiling must raise what one component may use, got {}",
            raised.per_component_default()
        );

        // And in the other direction: lowering the host total must not leave a
        // per-component default sitting above it.
        let lowered = MessagingLimits::resolve(Some(4), None, Some(1000));
        assert_eq!(lowered.host_total(), 4);
        assert!(
            lowered.per_component_default() <= lowered.host_total(),
            "per-component default {} exceeds the host total {} the operator set",
            lowered.per_component_default(),
            lowered.host_total()
        );
    }

    #[test]
    fn the_per_component_default_never_exceeds_the_host_total() {
        // The invariant the code documents, across every combination of "the
        // operator said something" and "the operator said nothing".
        for host in [None, Some(1), Some(4), Some(128), Some(1024)] {
            for pool in [None, Some(16), Some(1000), Some(64_000)] {
                let limits = MessagingLimits::resolve(host, None, pool);
                assert!(
                    limits.per_component_default() <= limits.host_total(),
                    "host={host:?} pool={pool:?} produced per_component {} > host {}",
                    limits.per_component_default(),
                    limits.host_total()
                );
            }
        }
    }

    #[test]
    fn a_small_pool_is_not_floored_above_its_own_capacity() {
        // The floor exists to keep a tiny host usable, but clamping *up* past
        // what the pool holds admits work the pool cannot instantiate — the
        // exact exhaustion these ceilings exist to prevent. A 16-instance pool
        // holds 3 components at the worst-case 5 core instances each, so the
        // ceiling may not be the unclamped floor of 8.
        for total in [4u32, 5, 16, 32] {
            let limits = MessagingLimits::resolve(None, None, Some(total));
            let admitted_core_instances =
                limits.host_total() * super::WORST_CASE_CORE_INSTANCES_PER_COMPONENT as usize;
            assert!(
                admitted_core_instances <= total as usize || limits.host_total() == 1,
                "pool of {total} core instances derived a ceiling of {} messages, \
                 which needs {admitted_core_instances} core instances",
                limits.host_total()
            );
        }
    }

    #[test]
    fn a_stock_pool_is_unaffected_by_the_floor() {
        // The floor only ever binds on a small pool; the stock 1000 must still
        // derive from its share, not from either bound.
        let limits = MessagingLimits::resolve(None, None, Some(1000));
        assert!(
            limits.host_total() > super::MIN_DERIVED_IN_FLIGHT
                && limits.host_total() < super::MAX_DERIVED_IN_FLIGHT,
            "the stock pool derived {}, which is one of the bounds, not its share",
            limits.host_total()
        );
    }

    #[test]
    fn the_documented_defaults_are_what_the_host_uses() {
        // Every user-facing surface quotes these numbers: the doc comments on
        // DEFAULT_MAX_IN_FLIGHT_HOST and DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT,
        // and the help for `--wasmcloud-messaging-max-in-flight` and
        // `--wasmcloud-messaging-max-in-flight-per-component`. They are two
        // different pairs — the pinned constants apply only where there is no
        // pool to divide — and the pooled pair is the one a stock `wash host`
        // actually runs with, so it is the one prose is most likely to get
        // wrong. Both are pinned here: changing the derivation, the pool share,
        // or either constant without rewriting the docs fails this test.
        let stock_pool = MessagingLimits::resolve(None, None, Some(1000));
        assert_eq!(stock_pool.host_total(), 133);
        assert_eq!(stock_pool.per_component_default(), 33);

        let unpooled = MessagingLimits::resolve(None, None, None);
        assert_eq!(unpooled.host_total(), 128);
        assert_eq!(unpooled.per_component_default(), 32);
    }

    #[test]
    fn the_pinned_defaults_stand_in_the_same_relationship_as_derived_ones() {
        // The values themselves are pinned by
        // `the_documented_defaults_are_what_the_host_uses`; what this adds is
        // the *relationship* between them. The pooled path divides the host
        // total by SATURATED_COMPONENTS_PER_HOST, so if the two pinned
        // constants ever drift out of that same ratio, the unpooled and pooled
        // paths stop meaning the same thing even while both still pass.
        assert_eq!(
            super::per_component_ceiling(DEFAULT_MAX_IN_FLIGHT_HOST),
            DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT
        );
    }

    #[test]
    fn an_explicit_config_value_parses_to_that_ceiling() {
        assert_eq!(super::parse_max_in_flight(Some("32")), Some(32));
        // Surrounding whitespace is the most likely YAML artifact.
        assert_eq!(super::parse_max_in_flight(Some(" 8 ")), Some(8));
    }

    #[test]
    fn an_explicit_value_is_honored() {
        let limits = MessagingLimits::new(128, 32);
        assert_eq!(limits.admission(&unique_identity(), Some(4)).limit(), 4);
        assert_eq!(limits.admission(&unique_identity(), Some(32)).limit(), 32);
    }

    #[test]
    fn a_component_may_not_exceed_the_per_component_ceiling() {
        // The flag is documented as "the most any single component may ask
        // for". Clamping only to the host total would leave it enforcing
        // nothing: a tenant manifest could name the entire host budget and
        // starve every co-tenant component on the host.
        let limits = MessagingLimits::new(128, 32);
        assert_eq!(
            limits.admission(&unique_identity(), Some(64)).limit(),
            32,
            "a component asking above the per-component ceiling must be clamped to it"
        );
        assert_eq!(
            limits.admission(&unique_identity(), Some(128)).limit(),
            32,
            "asking for the whole host budget must not grant it"
        );
        assert_eq!(
            limits
                .admission(&unique_identity(), Some(usize::MAX))
                .limit(),
            32,
            "nor must asking for everything representable"
        );
    }

    #[test]
    fn a_component_may_not_exceed_the_host_total() {
        // Clamped rather than rejected: a manifest sized against a bigger host
        // should still run on a smaller one, at the ceiling it can offer.
        let limits = MessagingLimits::new(16, 32);
        assert_eq!(limits.admission(&unique_identity(), Some(1024)).limit(), 16);
        // ...including via the default, when the default itself is the larger.
        assert_eq!(limits.admission(&unique_identity(), None).limit(), 16);
    }

    #[test]
    fn the_effective_ceiling_is_the_lower_of_the_two() {
        // Whichever way round the operator sets them, no component may exceed
        // either ceiling — the property that makes the pair a bound rather than
        // a suggestion.
        for (host, per_component) in [(128, 32), (32, 128), (64, 64), (1, 4096)] {
            let limits = MessagingLimits::new(host, per_component);
            let expected = host.min(per_component);
            for requested in [1, 8, 64, 4096, i32::MAX as usize] {
                let limit = limits
                    .admission(&unique_identity(), Some(requested))
                    .limit();
                assert!(
                    limit <= expected,
                    "host={host} per_component={per_component} requested={requested} \
                     resolved to {limit}, above the effective ceiling {expected}"
                );
            }
            assert_eq!(
                limits.admission(&unique_identity(), None).limit(),
                expected,
                "and so does the default"
            );
        }
    }

    #[test]
    fn zero_ceilings_floor_to_one_rather_than_meaning_unbounded() {
        // The CLI rejects an explicit zero outright; this is the belt-and-braces
        // floor for programmatic callers. Zero must never read as "no limit".
        let limits = MessagingLimits::new(0, 0);
        assert_eq!(limits.host_total(), 1);
        assert_eq!(limits.per_component_default(), 1);
        assert_eq!(limits.admission(&unique_identity(), None).limit(), 1);

        // Same floor on the requested side. `parse_max_in_flight` spells zero
        // as "unset" so this is unreachable from config, but a zero arriving
        // here would build a gate that admits nothing for the life of the
        // component — a silently dead subscriber rather than a small one.
        let limits = MessagingLimits::new(128, 32);
        assert_eq!(limits.admission(&unique_identity(), Some(0)).limit(), 1);
    }

    #[test]
    fn defaults_are_the_documented_pair() {
        let limits = MessagingLimits::default();
        assert_eq!(limits.host_total(), DEFAULT_MAX_IN_FLIGHT_HOST);
        assert_eq!(
            limits.per_component_default(),
            DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT
        );
        // The pairing the sizing arithmetic assumes: the per-component ceiling
        // is below the host total, so several components fit before the host
        // binds.
        const {
            assert!(DEFAULT_MAX_IN_FLIGHT_PER_COMPONENT < DEFAULT_MAX_IN_FLIGHT_HOST);
        }
    }

    #[tokio::test]
    async fn admission_bounds_one_component() {
        let limits = MessagingLimits::new(64, 32);
        let admission = limits.admission(&unique_identity(), Some(3));

        // Bound individually rather than collected: all three must be held at
        // once, and the release below must drop exactly one of them.
        let admit = || {
            admission
                .acquire()
                .now_or_never()
                .flatten()
                .expect("a free slot should admit immediately")
        };
        let first = admit();
        let _second = admit();
        let _third = admit();

        // Fourth must park rather than admit.
        assert!(
            admission.acquire().now_or_never().is_none(),
            "the ceiling must bind at 3"
        );

        // Releasing ONE frees exactly one slot.
        drop(first);
        assert!(
            admission.acquire().now_or_never().flatten().is_some(),
            "a completed handler must free its slot"
        );
    }

    /// The assertion the per-component test cannot make: three components each
    /// allowed 32 must still peak at the host total across all three, not 96.
    #[tokio::test]
    async fn the_host_total_bounds_every_component_together() {
        let limits = MessagingLimits::new(4, 32);
        let components: Vec<_> = (0..3)
            .map(|_| limits.admission(&unique_identity(), Some(32)))
            .collect();

        let mut held = Vec::new();
        // Round-robin so no single component could have taken all four.
        for round in 0..4 {
            let a = &components[round % components.len()];
            held.push(
                a.acquire()
                    .now_or_never()
                    .flatten()
                    .expect("within the host total"),
            );
        }

        for (i, a) in components.iter().enumerate() {
            assert!(
                a.acquire().now_or_never().is_none(),
                "component {i} must be blocked by the exhausted host total, \
                 despite having its own slots free"
            );
        }

        drop(held.pop());
        assert!(
            components[0].acquire().now_or_never().flatten().is_some(),
            "freeing a host slot must admit again"
        );
    }

    #[tokio::test]
    async fn a_permit_frees_both_levels() {
        // A host-permit leak is the worst failure mode here: it silently lowers
        // the ceiling for every component on the host, not just the leaker.
        let limits = MessagingLimits::new(1, 1);
        let a = limits.admission(&unique_identity(), Some(1));
        let b = limits.admission(&unique_identity(), Some(1));

        let permit = a.acquire().await.expect("first admits");
        assert!(
            b.acquire().now_or_never().is_none(),
            "the host total is exhausted by the other component"
        );

        drop(permit);
        assert!(
            b.acquire().now_or_never().flatten().is_some(),
            "dropping the permit must free the host slot too, not just the component one"
        );
    }

    #[tokio::test]
    async fn acquire_is_cancel_safe() {
        // Cancelling an acquire that is parked on the HOST semaphore must not
        // strand the component permit it already took. If it did, every
        // shutdown race would erode that component's ceiling by one.
        //
        // Host total 2 with a per-component default of 8 gives each component a
        // resolved ceiling of 2 (clamped), so the blocker can exhaust the host
        // while the waiter still has component slots free — the only shape in
        // which a waiter parks on the host level rather than its own.
        let limits = MessagingLimits::new(2, 8);
        let blocker = limits.admission(&unique_identity(), Some(8));
        let waiter = limits.admission(&unique_identity(), Some(8));
        assert_eq!(waiter.limit(), 2, "clamped to the host total");

        let held: Vec<_> = (0..2)
            .map(|_| blocker.acquire().now_or_never().flatten().expect("admits"))
            .collect();

        // Box::pin, not tokio::pin!: the latter rebinds the name to a
        // `Pin<&mut F>`, so dropping it would drop the *reference* and leave
        // the future — and its permit — alive on the stack.
        let mut parked = Box::pin(waiter.acquire());
        assert!(
            futures::poll!(parked.as_mut()).is_pending(),
            "the waiter has its own slots free, so it must be the host level parking it"
        );
        drop(parked);

        // With the host freed, the waiter must get its FULL ceiling back. One
        // short would mean the cancelled future kept its component permit.
        drop(held);
        let leak_check = || {
            waiter
                .acquire()
                .now_or_never()
                .flatten()
                .expect("no permit should have leaked from the cancelled acquire")
        };
        // Both held simultaneously: taking them one at a time would pass even
        // if only a single slot had come back.
        let (_a, _b) = (leak_check(), leak_check());
    }

    #[tokio::test]
    async fn a_closed_semaphore_ends_the_loop() {
        // How a shutting-down component tells its subscriber loop to stop.
        let limits = MessagingLimits::new(4, 4);
        let admission = limits.admission(&unique_identity(), Some(4));
        admission.component.close();
        assert!(
            admission.acquire().await.is_none(),
            "a closed component semaphore must report None, not park forever"
        );
    }

    #[test]
    fn ceilings_above_what_a_semaphore_holds_clamp_rather_than_panic() {
        // `Semaphore::new` panics above MAX_PERMITS, so an unchecked huge value
        // would abort the host at startup rather than act as a large ceiling.
        let limits = MessagingLimits::new(usize::MAX, usize::MAX);
        assert_eq!(limits.host_total(), MessagingLimits::MAX_IN_FLIGHT);
        assert_eq!(
            limits.per_component_default(),
            MessagingLimits::MAX_IN_FLIGHT
        );
        // And the gate it mints must be constructible too. The config value is
        // an unbounded `usize` now rather than a wire `i32`, so a request above
        // what a semaphore can hold has to clamp here as well.
        assert_eq!(
            limits
                .admission(&unique_identity(), Some(usize::MAX))
                .limit(),
            MessagingLimits::MAX_IN_FLIGHT,
            "a request above the maximum clamps rather than panicking"
        );
        assert_eq!(
            limits.admission(&unique_identity(), Some(4096)).limit(),
            4096,
            "an in-range request is still honored at the maximum"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_saturated_gate_sheds_at_the_deadline_rather_than_parking() {
        // Parking forever does not preserve the backlog: it stops the loop
        // draining the transport, and the transport drops on overflow with no
        // log and no metric. Shedding at a known deadline is what makes the
        // loss countable.
        let limits = MessagingLimits::new(1, 1).with_admission_wait(Duration::from_secs(5));
        let admission = limits.admission(&unique_identity(), Some(1));
        let _held = admission.acquire().await.expect("first admits");

        let outcome = admission.acquire_before_deadline("comp", "subj").await;
        assert!(
            matches!(outcome, Admitted::Shed),
            "a gate that never frees must shed, not park"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_slot_freed_inside_the_deadline_is_still_admitted() {
        // The deadline must not turn ordinary queueing into loss: a burst that
        // clears within the wait has to be processed, not shed.
        let limits = MessagingLimits::new(1, 1).with_admission_wait(Duration::from_secs(30));
        let admission = limits.admission(&unique_identity(), Some(1));
        let held = admission.acquire().await.expect("first admits");

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            drop(held);
        });

        let outcome = admission.acquire_before_deadline("comp", "subj").await;
        assert!(
            matches!(outcome, Admitted::Slot(_)),
            "a slot freed well inside the deadline must be admitted"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_still_reports_closed_rather_than_shedding() {
        // Shedding and shutting down are different outcomes: the loop must stop
        // on the latter, not drop one message and go round again.
        let limits = MessagingLimits::new(1, 1).with_admission_wait(Duration::from_secs(30));
        let admission = limits.admission(&unique_identity(), Some(1));
        // The clone stands in for the copy a subscriber loop holds: teardown
        // consumes the binding, the loop keeps running on its clone.
        let in_loop = admission.clone();
        // Through the same method teardown calls, so this covers the production
        // path rather than a semaphore only the test knows how to close.
        admission.close();
        assert!(
            matches!(
                in_loop.acquire_before_deadline("comp", "subj").await,
                Admitted::Closed
            ),
            "a closed semaphore must end the loop, not be mistaken for saturation"
        );
    }

    #[tokio::test]
    async fn closing_the_gate_wakes_a_loop_already_parked_on_it() {
        // The point of closing on teardown: a loop parked on a saturated gate
        // must not sit there until the deadline before noticing it is going
        // away. The 30s wait would make that obvious if it were still in force.
        let limits = MessagingLimits::new(1, 1).with_admission_wait(Duration::from_secs(30));
        let admission = limits.admission(&unique_identity(), Some(1));
        let _held = admission.acquire().await.expect("first admits");

        let parked = {
            let admission = admission.clone();
            tokio::spawn(async move { admission.acquire_before_deadline("comp", "subj").await })
        };
        // Let it reach the semaphore before closing underneath it.
        tokio::task::yield_now().await;
        admission.close();

        let outcome = tokio::time::timeout(Duration::from_secs(5), parked)
            .await
            .expect("closing must wake the parked loop well inside the deadline")
            .expect("task panicked");
        assert!(
            matches!(outcome, Admitted::Closed),
            "a parked loop must wake as Closed, not Shed"
        );
    }

    #[tokio::test]
    async fn replicas_of_one_deployment_share_a_gate() {
        // The whole point of keying by manifest identity. Each replica is a
        // separate workload with its own uuid component id, so keying by that
        // gave each one a full ceiling of its own: `replicas: 4` at
        // `max_in_flight: 32` could hold 128 messages on a host whose total is
        // 133. `max_in_flight` has to be a total for the component, however
        // many replicas of it happen to land here.
        let limits = MessagingLimits::new(64, 32);
        let identity = super::AdmissionIdentity::new("team-a", "ingester", "worker");
        let replica_a = limits.admission(&identity, Some(1));
        let replica_b = limits.admission(&identity, Some(1));

        let _held = replica_a.acquire().await.expect("first replica admits");
        assert!(
            replica_b.component.try_acquire().is_err(),
            "a second replica must contend for the same one-message ceiling, \
             not be handed a second one"
        );
    }

    #[tokio::test]
    async fn distinct_components_do_not_share_a_gate() {
        // The bound is per component, not per workload: two different
        // components of one workload must not contend, and neither must the
        // same component name in another namespace.
        let limits = MessagingLimits::new(64, 32);
        let ingester = limits.admission(
            &super::AdmissionIdentity::new("team-a", "pipeline", "ingester"),
            Some(1),
        );
        let indexer = limits.admission(
            &super::AdmissionIdentity::new("team-a", "pipeline", "indexer"),
            Some(1),
        );
        let other_tenant = limits.admission(
            &super::AdmissionIdentity::new("team-b", "pipeline", "ingester"),
            Some(1),
        );

        let _held = ingester.acquire().await.expect("admits");
        assert!(
            indexer.component.try_acquire().is_ok(),
            "a different component in the same workload must have its own gate"
        );
        assert!(
            other_tenant.component.try_acquire().is_ok(),
            "the same names in another namespace must not contend"
        );
    }

    #[tokio::test]
    async fn a_gate_closes_only_when_its_last_replica_goes_away() {
        // Sharing a gate makes close-on-teardown dangerous: closing when the
        // first replica goes away would wake a healthy replica's loop with
        // `Closed`, which breaks it out of its subscriber loop for good. The
        // refcount is what lets gate sharing and close-on-teardown coexist.
        let limits = MessagingLimits::new(64, 32);
        let identity = super::AdmissionIdentity::new("team-a", "ingester", "worker");
        let replica_a = limits.admission(&identity, Some(2));
        let replica_b = limits.admission(&identity, Some(2));
        assert_eq!(limits.gates.bindings(&identity), 2);

        replica_a.close();
        assert_eq!(limits.gates.bindings(&identity), 1);
        assert!(
            replica_b.acquire().await.is_some(),
            "a surviving replica must still be admitted after a sibling tears down"
        );

        replica_b.close();
        assert_eq!(
            limits.gates.bindings(&identity),
            0,
            "the last release must drop the entry rather than leaking it"
        );

        // And the gate really is closed now, so a loop still parked on it stops
        // rather than shedding.
        let rebound = limits.admission(&identity, Some(2));
        assert!(
            rebound.acquire().await.is_some(),
            "a later binding must get a fresh, open gate"
        );
    }

    #[test]
    fn only_the_component_gate_closes_not_the_shared_host_one() {
        // Closing the host semaphore would take every other component on the
        // host down with the one being torn down.
        let limits = MessagingLimits::new(4, 2);
        let going_away = limits.admission(&unique_identity(), Some(1));
        let survivor = limits.admission(&unique_identity(), Some(1));
        going_away.close();
        assert!(
            survivor.host.try_acquire().is_ok(),
            "host gate must survive"
        );
        assert!(
            survivor.component.try_acquire().is_ok(),
            "an unrelated component's gate must survive"
        );
    }

    #[test]
    fn a_shed_metric_is_attributed_to_manifest_identity_not_the_component_id() {
        // The component id is a fresh uuid per workload construction, so
        // attributing the counter with it mints a new series on every restart,
        // rolling update, and replica — cardinality driven by deployment churn,
        // and a value no operator can map back to a workload. Every attribute
        // here has to come from the manifest instead.
        let admission = MessagingLimits::default().admission(
            &super::AdmissionIdentity::new("team-a", "ingester", "worker"),
            None,
        );

        assert_eq!(&*admission.identity.namespace, "team-a");
        assert_eq!(&*admission.identity.workload, "ingester");
        assert_eq!(&*admission.identity.component, "worker");

        // The id passed per message is for the log line only; it must not reach
        // the identity the counter is split by.
        let uuid = uuid::Uuid::new_v4().to_string();
        assert!(
            [
                &*admission.identity.namespace,
                &*admission.identity.workload,
                &*admission.identity.component,
            ]
            .iter()
            .all(|v| *v != uuid),
            "a component id must never become a metric attribute"
        );
    }

    #[test]
    fn a_shed_metric_is_attributed_to_the_pattern_not_the_subject() {
        // Cardinality has to be bounded by configuration, not by traffic: a
        // wildcard subscription sees unboundedly many concrete subjects, and
        // the counter fires precisely during the flood that produces them.
        let limits = MessagingLimits::default();
        let admission = limits
            .admission(&unique_identity(), None)
            .with_subscriptions(&["orders.*.created".to_string(), "audit.>".to_string()]);

        assert_eq!(
            admission.subscription_for("orders.12345.created"),
            "orders.*.created"
        );
        assert_eq!(admission.subscription_for("audit.a.b.c"), "audit.>");
        // Never a value derived from the message itself.
        assert_eq!(
            admission.subscription_for("something.else"),
            super::UNMATCHED_SUBSCRIPTION
        );
    }

    #[test]
    fn no_configured_subscriptions_attributes_to_the_catch_all() {
        // An empty list means "everything"; that is one bounded value, not a
        // reason to fall back to the subject.
        let admission = MessagingLimits::default().admission(&unique_identity(), None);
        assert_eq!(admission.subscription_for("anything.at.all"), ">");
    }

    #[test]
    fn an_admission_wait_is_read_from_config() {
        // Durations in the spelling the rest of the CLI uses...
        assert_eq!(
            super::parse_admission_wait(Some("45s")),
            Some(Duration::from_secs(45))
        );
        assert_eq!(
            super::parse_admission_wait(Some("2m")),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            super::parse_admission_wait(Some("500ms")),
            Some(Duration::from_millis(500))
        );
        // ...and a bare number, which is what a plain-numeric-knob habit writes.
        assert_eq!(
            super::parse_admission_wait(Some("90")),
            Some(Duration::from_secs(90))
        );
        // Zero is a real policy ("do not queue"), not a spelling of unset.
        assert_eq!(super::parse_admission_wait(Some("0")), Some(Duration::ZERO));
        // Unset and malformed both fall back to the host default.
        for unset in [None, Some(""), Some("  "), Some("soon"), Some("5 parsecs")] {
            assert_eq!(super::parse_admission_wait(unset), None, "{unset:?}");
        }
    }

    #[test]
    fn an_admission_wait_cannot_become_forever() {
        // Waiting is bought with the transport's buffer, and the transport
        // drops on overflow without counting it. An unbounded wait therefore
        // trades countable sheds for uncountable loss — the exact failure the
        // bounded wait exists to prevent, reintroduced through a config knob.
        for too_long in ["1h", "24h", "3600", "18446744073709551615"] {
            assert_eq!(
                super::parse_admission_wait(Some(too_long)),
                Some(super::MAX_ADMISSION_WAIT),
                "{too_long} must clamp rather than park the loop indefinitely"
            );
        }
        // Clamped, not rejected: a manifest written for another host still runs.
        assert_eq!(
            super::parse_admission_wait(Some("5m")),
            Some(Duration::from_secs(300)),
            "a legitimately slow handler must still get the wait it asked for"
        );
        assert_eq!(
            super::parse_admission_wait(Some("10m")),
            Some(super::MAX_ADMISSION_WAIT),
            "the boundary itself is allowed through unchanged"
        );
    }

    #[test]
    fn every_setter_holds_the_admission_wait_bound() {
        // The bound has to be a property of the type, not of the config path.
        // `MessagingLimits::with_admission_wait` is how an embedder sets the
        // host-wide default, so clamping only in `parse_admission_wait` would
        // leave the unbounded park reachable from outside this crate.
        let over = Duration::from_secs(3600);
        assert_eq!(
            MessagingLimits::default()
                .with_admission_wait(over)
                .admission_wait(),
            super::MAX_ADMISSION_WAIT
        );
        assert_eq!(
            MessagingLimits::default()
                .admission(&unique_identity(), None)
                .with_admission_wait(Some(over))
                .wait(),
            super::MAX_ADMISSION_WAIT
        );
        // A wait inside the bound is still passed through untouched.
        let fine = Duration::from_secs(120);
        assert_eq!(
            MessagingLimits::default()
                .with_admission_wait(fine)
                .admission_wait(),
            fine
        );
    }

    #[test]
    fn a_configured_admission_wait_overrides_the_host_default() {
        let limits = MessagingLimits::default();
        assert_eq!(
            limits.admission(&unique_identity(), None).wait(),
            super::DEFAULT_ADMISSION_WAIT
        );
        assert_eq!(
            limits
                .admission(&unique_identity(), None)
                .with_admission_wait(Some(Duration::from_secs(300)))
                .wait(),
            Duration::from_secs(300),
            "a slow handler must be able to ask for a longer wait than the host default"
        );
        // Unset leaves the host default alone.
        assert_eq!(
            limits
                .admission(&unique_identity(), None)
                .with_admission_wait(None)
                .wait(),
            super::DEFAULT_ADMISSION_WAIT
        );
    }

    #[tokio::test]
    async fn permits_are_handed_out_in_arrival_order() {
        let limits = MessagingLimits::new(8, 1);
        let admission = limits.admission(&unique_identity(), Some(1));
        let held = admission.acquire().await.expect("first admits");

        let order = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for i in 0..3 {
            let admission = admission.clone();
            let order = Arc::clone(&order);
            // Stagger so the waiters queue in a known order.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            handles.push(tokio::spawn(async move {
                let _p = admission.acquire().await.expect("eventually admitted");
                (i, order.fetch_add(1, Ordering::SeqCst))
            }));
        }

        drop(held);
        let mut seen: Vec<(usize, usize)> = Vec::new();
        for h in handles {
            seen.push(h.await.expect("waiter task"));
        }
        seen.sort_by_key(|(_, position)| *position);
        assert_eq!(
            seen.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "tokio's Semaphore is FIFO-fair, so a hot component cannot starve the rest"
        );
    }
}
