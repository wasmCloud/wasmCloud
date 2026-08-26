//! Long-lived subscription loops spawned per workload.
//!
//! Three kinds of loops: JetStream push (with explicit ack via `MessageHandle`),
//! core NATS subscriptions (no ack), and KV watches. Each loop dispatches into
//! the component's split `wasmcloud:nats/*-handler` exports.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_nats::jetstream;
use futures::StreamExt;
use opentelemetry::KeyValue;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, error, warn};

use crate::engine::workload::ResolvedWorkload;
use crate::observability::FuelConsumptionMeter;
use crate::wasmtime::component::Resource;

use super::config::{AckMode, CoreSubscriptionConfig, JetStreamSubscriptionConfig, KvWatchConfig};
use super::conn::ConnHandle;
use super::handles::{BucketHandle, MessageHandle};
use super::{PLUGIN_NATS_ID, core_bindings, jetstream_bindings, kv_bindings};

use crate::engine::ctx::SharedCtx;

/// The pre-instantiated handler a component exports, and the instantiated
/// proxy a delivery is driven through.
///
/// Every `wasmcloud:nats` export is an `async func`, so a proxy binds through
/// wasmtime's concurrent ABI: its call takes an `Accessor` and must be driven
/// inside `run_concurrent`, which is also what lets the guest await its own
/// imports while the host keeps the store pumping.
macro_rules! handler_pre {
    ($name:ident, $proxy:ident, $pre:ty, $instance:ty) => {
        struct $name($pre);

        struct $proxy($instance);

        impl Clone for $name {
            fn clone(&self) -> Self {
                Self(self.0.clone())
            }
        }

        impl $name {
            fn new(
                instance_pre: crate::wasmtime::component::InstancePre<SharedCtx>,
            ) -> anyhow::Result<Self> {
                <$pre>::new(instance_pre).map(Self).map_err(|e| {
                    anyhow::anyhow!("component exports no wasmcloud:nats handler: {e}")
                })
            }

            async fn instantiate(
                &self,
                store: &mut crate::wasmtime::Store<SharedCtx>,
            ) -> anyhow::Result<$proxy> {
                Ok($proxy(self.0.instantiate_async(store).await?))
            }
        }
    };
}

handler_pre!(
    JsHandlerPre,
    JsProxy,
    jetstream_bindings::NatsJsProcessorPre<SharedCtx>,
    jetstream_bindings::NatsJsProcessor
);
handler_pre!(
    CoreHandlerPre,
    CoreProxy,
    core_bindings::NatsSubscriberPre<SharedCtx>,
    core_bindings::NatsSubscriber
);
handler_pre!(
    KvHandlerPre,
    KvProxy,
    kv_bindings::NatsKvWatcherPre<SharedCtx>,
    kv_bindings::NatsKvWatcher
);

impl JsProxy {
    async fn call(
        &self,
        store: &mut crate::wasmtime::Store<SharedCtx>,
        handle: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), String>> {
        let p = &self.0;
        store
            .run_concurrent(async move |accessor| {
                p.wasmcloud_nats_jetstream_handler()
                    .call_handle_message(accessor, handle)
                    .await
            })
            .await?
    }
}

impl CoreProxy {
    async fn call(
        &self,
        store: &mut crate::wasmtime::Store<SharedCtx>,
        raw: &async_nats::Message,
    ) -> wasmtime::Result<Result<(), String>> {
        let msg = core_bindings::wasmcloud::nats::types::NatsMessage {
            subject: raw.subject.to_string(),
            reply_to: raw.reply.as_ref().map(|r| r.to_string()),
            body: raw.payload.to_vec(),
            headers: raw.headers.as_ref().map(nats_headers_to_core_wit),
        };
        let p = &self.0;
        store
            .run_concurrent(async move |accessor| {
                p.wasmcloud_nats_core_handler()
                    .call_handle_message(accessor, msg)
                    .await
            })
            .await?
    }
}

impl KvProxy {
    async fn call(
        &self,
        store: &mut crate::wasmtime::Store<SharedCtx>,
        bucket: &str,
        entry: &jetstream::kv::Entry,
    ) -> wasmtime::Result<Result<(), String>> {
        let (bucket, entry) = (bucket.to_string(), kv_entry_to_kv_handler_wit(entry));
        let p = &self.0;
        store
            .run_concurrent(async move |accessor| {
                p.wasmcloud_nats_kv_handler()
                    .call_handle_event(accessor, bucket, entry)
                    .await
            })
            .await?
    }
}

/// Gives a fresh store fuel before anything runs in it.
///
/// With fuel metering enabled a store starts at zero, and instantiation runs
/// guest code — so without this the component traps on instantiate, before
/// `FuelConsumptionMeter::observe` gets a chance to set a budget. Errors when
/// metering is off, which is not a failure.
fn prime_fuel<T>(store: &mut crate::wasmtime::Store<T>) {
    let _ = store.set_fuel(u64::MAX);
}

/// How long an unsettled sequence holds a rebuild back before the loop stops
/// resuming at it.
///
/// A message that is genuinely still in play comes round again well inside
/// this — ack-wait is 30s and the redelivery backoff caps at 60s, each
/// delivery restarting the window — so what this bounds is the message that
/// never comes back at all: an ack the server accepted but the client reported
/// as failed, or one the stream's retention dropped. Without it such a sequence
/// would hold the resume point for the life of the subscription, and every
/// rebuild would replay the whole stream behind it.
const TRACKING_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// How many deliveries a sequence stays pinned in the in-flight set while it
/// goes unsettled.
///
/// The set is what a rebuilt consumer resumes from, so a message that never
/// settles — a poison body under `auto`, a guest under `manual` that never
/// acks — would otherwise hold the resume point at its sequence for the life of
/// the subscription, and every rebuild would replay the whole stream behind it.
/// The backoff reaches its 60s cap at delivery 31, so by this many the message
/// has been retried across roughly a quarter of an hour — far longer than the
/// outages a retry is for — and a rebuild that stops resuming at it gives up
/// little that was going to be handled. The server keeps redelivering it
/// either way; what stops is holding every later message's replay hostage to
/// it.
const MAX_TRACKED_DELIVERIES: u32 = 32;

/// Redelivery delay for a failed handler, growing with the delivery count.
///
/// Capped so a poison message settles into a slow retry rather than either
/// spinning or disappearing for hours.
fn redelivery_backoff(delivery_count: u32) -> std::time::Duration {
    const CAP_SECS: u64 = 60;
    let secs = u64::from(delivery_count.saturating_sub(1)).saturating_mul(2);
    std::time::Duration::from_secs(secs.clamp(1, CAP_SECS))
}

/// Heartbeat interval requested from push consumers.
///
/// Without it a consumer the server has forgotten — an ephemeral one after a
/// restart, say — leaves `messages().next()` parked forever with no error.
/// With it the stream surfaces `MissingHeartbeat` and the loop can rebuild.
///
/// Only ever asked for on a consumer with no deliver group. Heartbeats are
/// ordinary messages on the deliver subject, so a queue group load-balances
/// them the way it balances everything else: each one reaches a single member
/// and every other member's missed-heartbeat timer fires on a perfectly
/// healthy consumer. nats.go names the same trap `ErrNoHeartbeatForQueueSub`
/// and refuses the combination outright.
const IDLE_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(15);

/// How often a queue-group member asks the server whether its durable is still
/// there.
///
/// Queue members run without heartbeats, and `ConsumerDeleted` arrives on the
/// deliver subject — which is to say, to one member. This is what tells the
/// other members that the durable they are waiting on was deleted out from
/// under them.
const QUEUE_LIVENESS_PROBE: std::time::Duration = std::time::Duration::from_secs(30);

/// How long the server keeps an ephemeral push consumer with no live client.
/// Bounds what a workload leaves behind when it goes away without unbinding.
const EPHEMERAL_INACTIVE_THRESHOLD: std::time::Duration = std::time::Duration::from_secs(120);

/// Delay before rebuilding a push consumer whose delivery stream ended.
const RESUBSCRIBE_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

/// Ceiling on the rebuild backoff.
const RESUBSCRIBE_BACKOFF_CAP: std::time::Duration = std::time::Duration::from_secs(60);

/// Backoff for the `n`th consecutive failed rebuild, doubling to a cap.
///
/// A stream that is briefly absent — a server restart, provisioning that has
/// not caught up — comes back within seconds. One that is misconfigured never
/// does, and retrying it at a flat 2s is a permanent `STREAM.INFO` loop.
fn resubscribe_backoff(consecutive_failures: u32) -> std::time::Duration {
    RESUBSCRIBE_BACKOFF
        .saturating_mul(1u32 << consecutive_failures.min(5))
        .min(RESUBSCRIBE_BACKOFF_CAP)
}

/// Backoff applied when delivery setup fails.
///
/// Long enough that a persistent failure — a component that trap-loops on
/// instantiate, say — redelivers at a survivable rate rather than spinning,
/// and far shorter than letting the 30s ack-wait expire.
const SETUP_FAILURE_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);

/// Naks with a backoff so a failed setup neither stalls for the full ack-wait
/// nor spins: an immediate nak on a persistent failure is a hot loop.
async fn nak_with_backoff(acker: &async_nats::jetstream::message::Acker) {
    if let Err(e) = acker
        .ack_with(async_nats::jetstream::AckKind::Nak(Some(
            SETUP_FAILURE_BACKOFF,
        )))
        .await
    {
        warn!("failed to nak after a delivery setup failure: {e}");
    }
}

/// Folds a queue group into something a durable name and a subject token may
/// both carry.
///
/// The server treats a durable name as a token: `.`, `*`, `>` and whitespace
/// are rejected outright, and the same characters would split the deliver
/// subject into extra tokens. Two groups can fold together here, which is why
/// the name they end up in also carries a hash of the group as it was written.
fn sanitize_name_component(name: &str) -> String {
    let mut folded: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // All ASCII by now, so this cannot land mid-character.
    folded.truncate(32);
    folded
}

/// A short, stable hex digest over `parts`.
///
/// Not a security boundary: all it has to guarantee is that the same
/// subscription reaches the same name on every host and across restarts, and
/// that a subscription differing in any part reaches a different one. FNV-1a
/// is spelled out rather than reaching for `DefaultHasher` because
/// `DefaultHasher`'s output is explicitly not stable across Rust releases —
/// two hosts built with different toolchains would silently stop sharing a
/// queue group's durable.
fn short_hash(parts: &[&str]) -> String {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
        // A separator no part can contain, so ("a", "bc") and ("ab", "c") do
        // not digest alike.
        hash ^= 0xff;
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")[..12].to_string()
}

fn nats_headers_to_core_wit(
    headers: &async_nats::HeaderMap,
) -> Vec<core_bindings::wasmcloud::nats::types::HeaderEntry> {
    let mut out = Vec::new();
    for (name, values) in headers.iter() {
        for value in values {
            out.push(core_bindings::wasmcloud::nats::types::HeaderEntry {
                name: name.to_string(),
                value: value.as_str().to_string(),
            });
        }
    }
    out
}

fn kv_entry_to_kv_handler_wit(e: &jetstream::kv::Entry) -> kv_bindings::wasmcloud::nats::kv::Entry {
    let operation = match e.operation {
        jetstream::kv::Operation::Put => kv_bindings::wasmcloud::nats::kv::KvOperation::Put,
        jetstream::kv::Operation::Delete => kv_bindings::wasmcloud::nats::kv::KvOperation::Delete,
        jetstream::kv::Operation::Purge => kv_bindings::wasmcloud::nats::kv::KvOperation::Purge,
    };

    kv_bindings::wasmcloud::nats::kv::Entry {
        key: e.key.clone(),
        value: e.value.to_vec(),
        revision: e.revision,
        created_at_unix_nanos: e.created.unix_timestamp_nanos().max(0) as u64,
        operation,
    }
}

/// Spawn a JetStream push subscription per entry. Each consumer uses explicit
/// ack so the handler can decide ack / nak / term via the `message-handle`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn spawn_jetstream_subscriptions(
    workload: &ResolvedWorkload,
    component_id: &str,
    conn: Arc<ConnHandle>,
    subs: Vec<JetStreamSubscriptionConfig>,
    cancel_token: CancellationToken,
    fuel_meter: FuelConsumptionMeter,
    failure_sink: Option<crate::plugin::WorkloadFailureSink>,
    workload_id: impl Into<String>,
) -> anyhow::Result<()> {
    let instance_pre = workload.instantiate_pre(component_id).await?;
    let pre = JsHandlerPre::new(instance_pre)?;
    let workload_id = workload_id.into();
    // The durable and deliver plane is named per workload, not per host: N
    // hosts running one workload have to converge on one durable for the queue
    // group to mean anything, while a different workload with the same stream
    // and group must not join it.
    let scope = short_hash(&[workload_id.as_str()]);

    for sub in subs {
        let conn = conn.clone();
        let ack_mode = conn.ack_mode;
        let in_flight = Arc::new(tokio::sync::Semaphore::new(conn.limits.max_in_flight));
        let workload = workload.clone();
        let component_id = component_id.to_string();
        let pre = pre.clone();
        let cancel_token = cancel_token.clone();
        let fuel_meter = fuel_meter.clone();
        let failure_sink = failure_sink.clone();
        let workload_id = workload_id.clone();
        let scope = scope.clone();

        tokio::spawn(async move {
            // Mark the current generation as seen, so the connect that opened
            // this connection does not immediately count as a reconnect.
            let mut reconnects = conn.reconnects.subscribe();
            reconnects.mark_unchanged();
            // A SUB the server refuses is refused asynchronously: the deliver
            // subject looks subscribed and simply never carries anything.
            let mut denials = conn.subscription_denials.subscribe();
            let mut denials_reported: std::collections::HashSet<String> =
                std::collections::HashSet::new();

            let configured_policy = match sub.deliver_policy.as_str() {
                "all" => jetstream::consumer::DeliverPolicy::All,
                "last" => jetstream::consumer::DeliverPolicy::Last,
                "last-per-subject" => jetstream::consumer::DeliverPolicy::LastPerSubject,
                _ => jetstream::consumer::DeliverPolicy::New,
            };

            // Sequences dispatched into the component but not yet settled, and
            // the highest one seen. A rebuilt ephemeral consumer starts from
            // the oldest of those: the original policy would either replay the
            // whole stream (`all`) or drop everything published during the
            // outage (`new`), and the destroyed consumer took its ack state
            // with it, so anything unsettled has to come round again.
            //
            // A sequence leaves the set when the server confirms it settled,
            // when it has been redelivered `MAX_TRACKED_DELIVERIES` times
            // without ever settling, or when `TRACKING_TTL` passes with no
            // further delivery. The last two bound what one message that never
            // settles can cost: while it is tracked, every rebuild replays the
            // stream behind it.
            let in_flight_sequences: InFlight =
                Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
            let mut last_delivered: Option<u64> = None;
            // Only an ephemeral consumer resumes from a tracked position: a
            // queue-group consumer is durable and keeps its own server-side, so
            // tracking there would grow a set nothing ever reads.
            let tracks_sequences = sub.queue_group.is_none();
            // The stream's creation time, so a stream that was deleted and
            // recreated under us is recognised as a different stream: its
            // sequences restart at 1 and our old position means nothing.
            let mut stream_created: Option<i128> = None;
            let mut consecutive_failures = 0u32;
            // The ephemeral consumer the last cycle built, so the next one can
            // take it off the server rather than leave it to age out. A loop
            // that can never deliver rebuilds indefinitely, and each cycle
            // would otherwise park another consumer for its inactive
            // threshold.
            let mut previous_ephemeral: Option<String> = None;

            // Rebuild the consumer whenever delivery stops. An ephemeral push
            // consumer does not survive a server restart, and without this the
            // subscription parks forever instead of coming back.
            'delivery: loop {
                // Attach only. Creating the stream here would let a
                // subscription provision arbitrary streams outside the grant;
                // the interface contract is that streams are provisioned
                // out-of-band.
                let stream = match conn.jetstream.get_stream(&sub.stream).await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(stream = %sub.stream, "JetStream stream unavailable: {e}");
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        if wait_or_cancel(&cancel_token, resubscribe_backoff(consecutive_failures))
                            .await
                        {
                            break 'delivery;
                        }
                        continue 'delivery;
                    }
                };

                // The previous cycle's ephemeral is ours alone and nothing is
                // waiting on it, so it goes now rather than lingering for its
                // inactive threshold. Best effort: after a server restart it
                // is already gone, which is not worth a warn.
                if let Some(name) = previous_ephemeral.take()
                    && let Err(e) = stream.delete_consumer(&name).await
                {
                    debug!(stream = %sub.stream, "could not delete the previous ephemeral consumer: {e}");
                }

                // With a queue group the consumer must be shared across hosts,
                // or each host creates its own and every host sees every
                // message. Without one it stays ephemeral and per-host, and
                // gets a fresh deliver subject on every rebuild.
                //
                // The shared name carries more than the group: a durable is a
                // rendezvous, and anything that disagrees about what it should
                // deliver has to rendezvous somewhere else. Hashing the stream,
                // the filter, the policy and the group means two hosts running
                // the same subscription still meet on one durable, while a
                // second subscription that merely reuses the group name — a
                // different workload, or the old version mid-rollout — gets
                // its own instead of rewriting this one's filter out from
                // under it.
                let (durable_name, deliver_subject, inactive_threshold) = match &sub.queue_group {
                    Some(group) => {
                        let folded = sanitize_name_component(group);
                        let identity = short_hash(&[
                            &sub.stream,
                            &sub.filter_subject,
                            &sub.deliver_policy,
                            group,
                        ]);
                        (
                            Some(format!("{folded}_{scope}_{identity}")),
                            format!("_nats_push.{scope}.{folded}.{identity}"),
                            std::time::Duration::ZERO,
                        )
                    }
                    None => (
                        None,
                        format!("_nats_push.{}", uuid::Uuid::new_v4()),
                        EPHEMERAL_INACTIVE_THRESHOLD,
                    ),
                };

                // Only an ephemeral consumer needs a resume point; a durable
                // one keeps its own position server-side.
                let resume_from = match sub.queue_group {
                    Some(_) => None,
                    None => oldest_tracked(&in_flight_sequences)
                        .or_else(|| last_delivered.map(|seq| seq.saturating_add(1)))
                        // Stream sequences start at 1, so 0 is not a position
                        // the server will accept as a start: asking for it
                        // fails the create and takes the whole subscription
                        // down with it.
                        .filter(|&sequence| sequence >= 1),
                };
                let info = stream.cached_info();
                let created = info.created.unix_timestamp_nanos();
                let recreated = stream_created.is_some_and(|previous| previous != created);
                stream_created = Some(created);
                if recreated {
                    // Sequences restart at 1 in the new stream, so every
                    // position held for the old one is meaningless — and a
                    // `last_delivered` from the old stream would sit past the
                    // new stream's end, sending every later rebuild back to
                    // `first_sequence` to replay the whole thing again.
                    last_delivered = None;
                    if let Ok(mut pending) = in_flight_sequences.lock() {
                        pending.clear();
                    }
                }
                let deliver_policy = match resume_from {
                    // Nothing was ever delivered, so there is no position to
                    // resume from: the configured policy still applies.
                    None => configured_policy,
                    // Same stream, and our position is still within it.
                    Some(start_sequence)
                        if !recreated && start_sequence <= info.state.last_sequence + 1 =>
                    {
                        jetstream::consumer::DeliverPolicy::ByStartSequence {
                            start_sequence: start_sequence.max(1),
                        }
                    }
                    // The stream was replaced, or rolled out from under us.
                    // Take what it holds rather than skipping past it.
                    Some(_) => jetstream::consumer::DeliverPolicy::ByStartSequence {
                        start_sequence: info.state.first_sequence.max(1),
                    },
                };

                let config = jetstream::consumer::push::Config {
                    durable_name: durable_name.clone(),
                    filter_subject: sub.filter_subject.clone(),
                    deliver_subject: deliver_subject.clone(),
                    deliver_group: sub.queue_group.clone(),
                    ack_policy: jetstream::consumer::AckPolicy::Explicit,
                    ack_wait: std::time::Duration::from_secs(30),
                    deliver_policy,
                    // Server-enforced ceiling on unacked deliveries: it
                    // stops sending past this, pacing the stream to how fast
                    // the component drains. A queue group shares one durable,
                    // so replicas divide this budget.
                    max_ack_pending: conn.limits.effective_max_ack_pending(),
                    // A queue group is deliberately left without one; see
                    // `IDLE_HEARTBEAT`. `QUEUE_LIVENESS_PROBE` covers what the
                    // heartbeat would have caught there.
                    idle_heartbeat: match sub.queue_group {
                        Some(_) => std::time::Duration::ZERO,
                        None => IDLE_HEARTBEAT,
                    },
                    inactive_threshold,
                    ..Default::default()
                };

                let consumer = match durable_name.as_deref() {
                    // A durable is a rendezvous, so the create is strict:
                    // create-or-update would let whoever spawned last quietly
                    // rewrite the config every peer is already being served
                    // under. Finding it already there is the ordinary case —
                    // that is another host of this workload — so attach, and
                    // check that what is there is what this subscription
                    // asked for rather than assume it.
                    Some(durable) => match stream.create_consumer_strict(config.clone()).await {
                        Ok(c) => c,
                        Err(e)
                            if matches!(
                                e.kind(),
                                jetstream::stream::ConsumerCreateStrictErrorKind::AlreadyExists
                            ) =>
                        {
                            match stream
                                .get_consumer::<jetstream::consumer::push::Config>(durable)
                                .await
                            {
                                Ok(existing) => {
                                    let found = &existing.cached_info().config;
                                    if found.filter_subject != config.filter_subject
                                        || found.deliver_policy != config.deliver_policy
                                    {
                                        error!(
                                            durable,
                                            stream = %sub.stream,
                                            found_filter = %found.filter_subject,
                                            wanted_filter = %config.filter_subject,
                                            "a consumer under this name already delivers \
                                             something else; refusing to rewrite it"
                                        );
                                        consecutive_failures =
                                            consecutive_failures.saturating_add(1);
                                        if wait_or_cancel(
                                            &cancel_token,
                                            resubscribe_backoff(consecutive_failures),
                                        )
                                        .await
                                        {
                                            break 'delivery;
                                        }
                                        continue 'delivery;
                                    }
                                    existing
                                }
                                Err(e) => {
                                    warn!(
                                        durable,
                                        "failed to attach to the existing push consumer: {e}"
                                    );
                                    consecutive_failures = consecutive_failures.saturating_add(1);
                                    if wait_or_cancel(
                                        &cancel_token,
                                        resubscribe_backoff(consecutive_failures),
                                    )
                                    .await
                                    {
                                        break 'delivery;
                                    }
                                    continue 'delivery;
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "failed to create push consumer for '{}': {e}",
                                sub.filter_subject
                            );
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            if wait_or_cancel(
                                &cancel_token,
                                resubscribe_backoff(consecutive_failures),
                            )
                            .await
                            {
                                break 'delivery;
                            }
                            continue 'delivery;
                        }
                    },
                    None => match stream.create_consumer(config).await {
                        Ok(c) => c,
                        Err(e) => {
                            warn!(
                                "failed to create push consumer for '{}': {e}",
                                sub.filter_subject
                            );
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            if wait_or_cancel(
                                &cancel_token,
                                resubscribe_backoff(consecutive_failures),
                            )
                            .await
                            {
                                break 'delivery;
                            }
                            continue 'delivery;
                        }
                    },
                };

                if durable_name.is_none() {
                    previous_ephemeral = Some(consumer.cached_info().name.clone());
                }

                let mut messages = match consumer.messages().await {
                    Ok(m) => m,
                    Err(e) => {
                        warn!(
                            "failed to get message stream for '{}': {e}",
                            sub.filter_subject
                        );
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        if wait_or_cancel(&cancel_token, resubscribe_backoff(consecutive_failures))
                            .await
                        {
                            break 'delivery;
                        }
                        continue 'delivery;
                    }
                };

                // A rebuild that never delivered anything is not a recovery,
                // and treating it as one keeps the ladder at its first rung:
                // a subscription the server will never deliver on — a deliver
                // subject the workload's credentials forbid, say — would
                // rebuild at a flat 2s for the life of the host.
                let mut delivered_this_cycle = false;

                // Queue members have no heartbeat to miss, so this is what
                // notices a durable that was deleted out from under them.
                let mut liveness = tokio::time::interval(QUEUE_LIVENESS_PROBE);
                // A busy loop polls this late and a stalled one not at all;
                // either way the probe is a liveness check, not a schedule to
                // catch up on.
                liveness.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                liveness.tick().await;

                loop {
                    // The permit comes first so that nothing is taken off the
                    // consumer that the loop is not ready to run. Waiting for
                    // one inside the delivery arm parks the loop where it can
                    // see neither cancellation nor a reconnect, holds a
                    // message against its ack-wait, and — when the permit
                    // finally frees after an unbind — dispatches into a
                    // workload that is already gone. Idling with a permit in
                    // hand costs nothing: the semaphore is this
                    // subscription's alone, and the next message would take
                    // this permit anyway.
                    let permit = tokio::select! {
                        acquired = in_flight.clone().acquire_owned() => match acquired {
                            Ok(p) => p,
                            Err(_) => break 'delivery,
                        },
                        _ = reconnects.changed() => {
                            warn!(
                                stream = %sub.stream,
                                "reconnected to NATS, rebuilding push consumer"
                            );
                            break;
                        }
                        _ = cancel_token.cancelled() => break 'delivery,
                    };

                    tokio::select! {
                        maybe_msg = messages.next() => {
                            let raw = match maybe_msg {
                                None => {
                                    warn!(
                                        stream = %sub.stream,
                                        "JetStream delivery stream ended, rebuilding consumer"
                                    );
                                    break;
                                }
                                Some(Err(e)) => {
                                    // A missed heartbeat or a deleted consumer
                                    // means this delivery path is dead; anything
                                    // else is transient.
                                    if matches!(
                                        e.kind(),
                                        jetstream::consumer::push::MessagesErrorKind::MissingHeartbeat
                                            | jetstream::consumer::push::MessagesErrorKind::ConsumerDeleted
                                    ) {
                                        warn!(
                                            stream = %sub.stream,
                                            "JetStream delivery lost, rebuilding consumer: {e}"
                                        );
                                        break;
                                    }
                                    warn!(
                                        stream = %sub.stream,
                                        "transient JetStream delivery error, continuing: {e}"
                                    );
                                    continue;
                                }
                                Some(Ok(m)) => m,
                            };

                            delivered_this_cycle = true;

                            let subject_str = raw.subject.to_string();
                            let (sequence, delivery_count) = match raw.info() {
                                Ok(i) => (i.stream_sequence, i.delivered as u32),
                                // Nothing about this delivery can be trusted:
                                // without its sequence there is no position to
                                // ack at, to resume from, or to hand the guest.
                                // Anything the host invented here would reach
                                // the component as a genuine stream message and
                                // poison the rebuild resume point besides, so
                                // it goes back to the server instead.
                                Err(e) => {
                                    let (_, acker) = raw.split();
                                    warn!(
                                        subject = %subject_str,
                                        stream = %sub.stream,
                                        "failed to parse JetStream metadata; naking: {e}"
                                    );
                                    nak_with_backoff(&acker).await;
                                    continue;
                                }
                            };
                            let (message, acker) = raw.split();

                            // Belt and braces: a metadata parse failure never
                            // reaches here any more, and 0 is neither a
                            // sequence the server issues nor one it accepts
                            // back as a start position.
                            if sequence != 0 {
                                last_delivered = Some(
                                    last_delivered.map_or(sequence, |seen| seen.max(sequence)),
                                );
                                if tracks_sequences {
                                    if delivery_count < MAX_TRACKED_DELIVERIES {
                                        track_sequence(&in_flight_sequences, sequence);
                                    } else {
                                        // It has had every try the backoff will
                                        // give it. Stop holding the resume point
                                        // here, or one poison message costs every
                                        // later message a replay on each rebuild.
                                        // Said once: it keeps being redelivered.
                                        if delivery_count == MAX_TRACKED_DELIVERIES {
                                            warn!(
                                                sequence,
                                                delivery_count,
                                                stream = %sub.stream,
                                                "JetStream message still unsettled after \
                                                 {MAX_TRACKED_DELIVERIES} deliveries; a rebuilt \
                                                 consumer will no longer resume at it"
                                            );
                                        }
                                        release_sequence(&in_flight_sequences, sequence);
                                    }
                                }
                            }
                            let settled_sequences = in_flight_sequences.clone();

                            // Both of these run guest code — instantiation is
                            // guest code, and a store that cannot be made is
                            // usually a host under pressure — so both are
                            // raced against cancellation. A message caught
                            // mid-setup by an unbind goes back to the server
                            // rather than burning its ack-wait against a
                            // workload that no longer exists.
                            let mut store = tokio::select! {
                                created = workload.new_store(&component_id) => match created {
                                    Err(e) => {
                                        warn!("failed to create store for {component_id}: {e}");
                                        // Nak'd, not settled: the sequence stays in
                                        // the in-flight set so a rebuilt consumer
                                        // resumes at it rather than past it.
                                        nak_with_backoff(&acker).await;
                                        continue;
                                    }
                                    Ok(s) => s,
                                },
                                _ = cancel_token.cancelled() => {
                                    nak_with_backoff(&acker).await;
                                    release_sequence(&in_flight_sequences, sequence);
                                    break 'delivery;
                                }
                            };
                            prime_fuel(&mut store);
                            let proxy = tokio::select! {
                                instantiated = pre.instantiate(&mut store) => match instantiated {
                                    Err(e) => {
                                        warn!("failed to instantiate {component_id}: {e}");
                                        nak_with_backoff(&acker).await;
                                        continue;
                                    }
                                    Ok(p) => p,
                                },
                                _ = cancel_token.cancelled() => {
                                    nak_with_backoff(&acker).await;
                                    release_sequence(&in_flight_sequences, sequence);
                                    break 'delivery;
                                }
                            };

                            // One `Acker`, three uses: whoever settles keeps
                            // `acker`, and `progress` extends ack-wait without
                            // settling anything, which both modes need.
                            let acker = std::sync::Arc::new(acker);
                            // Under `auto` the host settles the message, so the
                            // guest handle carries no settling acker.
                            let (guest_acker, host_acker) = match ack_mode {
                                AckMode::Auto => (None, Some(acker.clone())),
                                AckMode::Manual => (Some(acker.clone()), None),
                            };
                            // Under `manual` this is how the guest's ack or
                            // term reaches the loop: nothing else survives the
                            // store, which is dropped with the handler task.
                            let guest_settled = Arc::new(AtomicBool::new(false));
                            // A handler answering a request publishes to the
                            // inbox the requester chose, which no sane grant
                            // covers. This is what authorizes that one reply,
                            // for this inbox only.
                            if let Some(reply) = message.reply.as_deref() {
                                conn.grant_reply(reply);
                            }
                            let handle = MessageHandle {
                                acker: guest_acker,
                                progress: Some(acker.clone()),
                                settled: guest_settled.clone(),
                                message,
                                sequence,
                                delivery_count,
                            };
                            let resource: Resource<MessageHandle> =
                                match store.data_mut().table.push(handle) {
                                    Ok(r) => r,
                                    Err(e) => {
                                        warn!("failed to push message-handle for {component_id}: {e}");
                                        // The handle was dropped, so the guest
                                        // never received this and cannot settle
                                        // it. Nak in both modes: waiting out the
                                        // 30s ack-wait instead would crawl under
                                        // exactly the table pressure that caused
                                        // this, while every sibling setup
                                        // failure redelivers in 5s.
                                        nak_with_backoff(&acker).await;
                                        continue;
                                    }
                                };

                            let span = tracing::span!(
                                tracing::Level::INFO,
                                "incoming_nats_jetstream_message",
                                subject = %subject_str,
                                sequence,
                                stream = %sub.stream,
                            );

                            let fuel_meter = fuel_meter.clone();
                            let subject_label = subject_str.clone();
                            tokio::spawn(async move {
                                let result = fuel_meter.observe(
                                    &[
                                        KeyValue::new("plugin", PLUGIN_NATS_ID),
                                        KeyValue::new("subject", subject_label),
                                    ],
                                    &mut store,
                                    async move |store| {
                                        proxy
                                            .call(store, resource)
                                            .instrument(span)
                                            .await
                                            .map_err(Into::into)
                                    },
                                ).await;
                                // The handler's own error is the useful one; the
                                // outer result only carries traps.
                                match &result {
                                    Ok(Err(handler_err)) => warn!(
                                        subject = %subject_str,
                                        sequence,
                                        delivery_count,
                                        "JetStream handler returned an error: {handler_err}"
                                    ),
                                    Err(e) => warn!(
                                        subject = %subject_str,
                                        sequence,
                                        "JetStream handler trapped: {e}"
                                    ),
                                    Ok(Ok(())) => {}
                                }
                                // The component is done. Dropping the store frees
                                // the instance and its memory before the slot, so
                                // the next message cannot start while this one is
                                // still resident; holding either across the ack
                                // below would put a round trip between each
                                // message and the next.
                                drop(store);
                                drop(permit);

                                // Under `auto`, the handler's outcome decides the
                                // ack. A trap or an `Err` naks so the message is
                                // redelivered, with a backoff that grows by
                                // delivery count: a handler that fails permanently
                                // would otherwise spin as fast as the server can
                                // redeliver.
                                let retired = match host_acker {
                                    Some(acker) => {
                                        let handled = matches!(&result, Ok(Ok(())));
                                        let settled = if handled {
                                            // A plain `ack` is a fire-and-forget
                                            // publish: it can be lost in the
                                            // very disconnect that forces the
                                            // rebuild, and the sequence would
                                            // then be retired on the strength of
                                            // an ack the server never saw. The
                                            // round trip costs far less than the
                                            // instantiation that just ran.
                                            acker.double_ack().await
                                        } else {
                                            acker
                                                .ack_with(async_nats::jetstream::AckKind::Nak(Some(
                                                    redelivery_backoff(delivery_count),
                                                )))
                                                .await
                                        };
                                        if let Err(e) = &settled {
                                            warn!("failed to settle JetStream message: {e}");
                                        }
                                        handled && settled.is_ok()
                                    }
                                    // Under `manual` the guest owns the ack, so
                                    // only its own ack or term retires this.
                                    None => guest_settled.load(Ordering::Acquire),
                                };
                                // Only a message the server has taken off our
                                // hands leaves the in-flight set. A nak, a
                                // failed ack, or a guest that returned without
                                // settling is still coming round again, and the
                                // set is what a rebuild resumes from — dropping
                                // it here is how a redelivery gets skipped.
                                if retired {
                                    release_sequence(&settled_sequences, sequence);
                                }
                            });
                        }
                        // Without heartbeats there is nothing to miss, so a
                        // queue member asks after its durable itself. Only the
                        // durable it is actually attached to is worth asking
                        // about; an ephemeral has its heartbeat.
                        _ = liveness.tick(), if durable_name.is_some() => {
                            if let Some(durable) = durable_name.as_deref()
                                && let Err(e) = stream.consumer_info(durable).await
                                && matches!(
                                    e.kind(),
                                    jetstream::context::ConsumerInfoErrorKind::NotFound
                                )
                            {
                                warn!(
                                    durable,
                                    stream = %sub.stream,
                                    "the durable consumer is gone, rebuilding it: {e}"
                                );
                                break;
                            }
                        }
                        // The server accepted the SUB on the deliver subject
                        // and then refused it, so nothing will ever arrive on
                        // it. Rebuilding cannot help — only the workload's
                        // credentials can.
                        denied = denials.recv() => {
                            if let Ok(subject) = denied
                                && subject == deliver_subject
                                && denials_reported.insert(subject.clone())
                            {
                                let reason = format!(
                                    "NATS server denied SUB on the JetStream deliver subject \
                                     '{subject}'; the workload's credentials must allow \
                                     subscribing to '_nats_push.>' for push delivery"
                                );
                                error!(stream = %sub.stream, "{reason}");
                                if let Some(sink) = &failure_sink {
                                    sink.report(workload_id.clone(), reason);
                                }
                            }
                        }
                        // A reconnect means the server may have restarted, and
                        // an ephemeral consumer does not survive that. The
                        // delivery subject stays subscribed either way, so
                        // nothing else would ever report it.
                        _ = reconnects.changed() => {
                            warn!(
                                stream = %sub.stream,
                                "reconnected to NATS, rebuilding push consumer"
                            );
                            break;
                        }
                        _ = cancel_token.cancelled() => break 'delivery,
                    }
                }

                consecutive_failures = if delivered_this_cycle {
                    0
                } else {
                    consecutive_failures.saturating_add(1)
                };
                if wait_or_cancel(&cancel_token, resubscribe_backoff(consecutive_failures)).await {
                    break 'delivery;
                }
            }
        });
    }

    Ok(())
}

/// Sequences dispatched into a component but not yet settled, each with the
/// instant after which it stops holding a rebuild back.
type InFlight = Arc<std::sync::Mutex<std::collections::BTreeMap<u64, tokio::time::Instant>>>;

/// Drops a sequence from the in-flight set, so a rebuild does not rewind to it.
fn release_sequence(sequences: &InFlight, sequence: u64) {
    if let Ok(mut pending) = sequences.lock() {
        pending.remove(&sequence);
    }
}

/// Starts (or extends) the window in which `sequence` holds a rebuild back.
///
/// Prunes as it goes: a sequence whose settle the server accepted but never
/// confirmed is released by nothing else, and a subscription that never
/// rebuilds would otherwise carry those entries for its whole life.
fn track_sequence(sequences: &InFlight, sequence: u64) {
    if let Ok(mut pending) = sequences.lock() {
        let now = tokio::time::Instant::now();
        pending.retain(|_, deadline| *deadline > now);
        pending.insert(sequence, now + TRACKING_TTL);
    }
}

/// The oldest sequence still worth resuming at, dropping any whose window has
/// closed.
fn oldest_tracked(sequences: &InFlight) -> Option<u64> {
    let mut pending = sequences.lock().ok()?;
    let now = tokio::time::Instant::now();
    pending.retain(|_, deadline| *deadline > now);
    pending.keys().next().copied()
}

/// Sleeps for `delay`, returning true if the workload was cancelled instead.
async fn wait_or_cancel(cancel_token: &CancellationToken, delay: std::time::Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        _ = cancel_token.cancelled() => true,
    }
}

/// How often a core subscription that is shedding reports its running totals.
///
/// Rate-limited rather than per-dropped-message: shedding happens at rates
/// that would otherwise flood the log with identical lines.
const SHED_REPORT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// What a core subscription has queued and what it has had to shed.
///
/// Bytes are tracked because `subscription-capacity` counts messages, and a
/// large-payload burst exhausts memory. The reader only adds and the delivery
/// loop only subtracts, so the count runs slightly conservative at worst.
#[derive(Default)]
struct CoreBacklog {
    /// Bytes sitting between the reader and the delivery loop.
    queued_bytes: std::sync::atomic::AtomicUsize,
    /// Deliveries shed since the subscription started, and their total size.
    shed: std::sync::atomic::AtomicU64,
    shed_bytes: std::sync::atomic::AtomicU64,
}

/// Whether a delivery of `len` bytes may join a backlog already holding
/// `queued`.
///
/// An empty backlog always admits, so a byte budget smaller than a single
/// message shrinks throughput rather than shedding everything forever.
fn admits(queued: usize, len: usize, max_bytes: usize) -> bool {
    queued == 0 || queued.saturating_add(len) <= max_bytes
}

/// Records one shed delivery, reporting no more than once per interval.
fn record_shed(
    backlog: &CoreBacklog,
    subject: &str,
    len: usize,
    last_report: &mut Option<tokio::time::Instant>,
    reason: &'static str,
) {
    let total = backlog.shed.fetch_add(1, Ordering::Relaxed) + 1;
    let total_bytes = backlog.shed_bytes.fetch_add(len as u64, Ordering::Relaxed) + len as u64;
    let now = tokio::time::Instant::now();
    if last_report.is_none_or(|at| now.duration_since(at) >= SHED_REPORT_INTERVAL) {
        *last_report = Some(now);
        warn!(
            subject = %subject,
            reason,
            shed_total = total,
            shed_bytes = total_bytes,
            queued_bytes = backlog.queued_bytes.load(Ordering::Relaxed),
            "core subscription is shedding: deliveries are arriving faster than \
             the component drains them"
        );
    }
}

/// Moves deliveries off the NATS subscription and into the host's own backlog.
///
/// `async_nats::Subscriber` silently discards messages when its channel fills,
/// so a delivery loop that stops reading while waiting for a handler permit
/// loses messages it cannot count. A task that does nothing but drain keeps
/// that channel near empty and parks the backlog here, where shedding is
/// counted, attributed to a subject, and bounded in bytes.
fn spawn_core_reader(
    subject: String,
    mut messages: async_nats::Subscriber,
    deliveries: tokio::sync::mpsc::Sender<async_nats::Message>,
    backlog: Arc<CoreBacklog>,
    max_bytes: usize,
    cancel_token: CancellationToken,
) {
    tokio::spawn(async move {
        let mut last_report = None;
        loop {
            // Without the cancellation arm a torn-down workload leaves this
            // task parked on a subscription that may never deliver again: the
            // closed channel is only noticed on the next message to arrive.
            let message = tokio::select! {
                next = messages.next() => match next {
                    Some(m) => m,
                    None => break,
                },
                _ = cancel_token.cancelled() => break,
            };

            let len = message.length;
            let queued = backlog.queued_bytes.load(Ordering::Relaxed);
            if !admits(queued, len, max_bytes) {
                record_shed(&backlog, &subject, len, &mut last_report, "byte budget");
                continue;
            }

            match deliveries.try_send(message) {
                Ok(()) => {
                    backlog.queued_bytes.fetch_add(len, Ordering::Relaxed);
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    record_shed(&backlog, &subject, len, &mut last_report, "backlog full");
                }
                // The delivery loop is gone; so is the reason to read.
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
            }
        }

        // Dropping the subscriber closes its channel, which is what the client
        // turns into an UNSUB.
        let shed = backlog.shed.load(Ordering::Relaxed);
        if shed > 0 {
            warn!(
                subject = %subject,
                shed_total = shed,
                shed_bytes = backlog.shed_bytes.load(Ordering::Relaxed),
                "core subscription ended having shed deliveries"
            );
        }
    });
}

/// Spawn core NATS subscribers (no ack semantics, optional queue group).
#[allow(clippy::too_many_arguments)]
pub(super) async fn spawn_core_subscriptions(
    workload: &ResolvedWorkload,
    component_id: &str,
    conn: Arc<ConnHandle>,
    subs: Vec<CoreSubscriptionConfig>,
    cancel_token: CancellationToken,
    fuel_meter: FuelConsumptionMeter,
    failure_sink: Option<crate::plugin::WorkloadFailureSink>,
    workload_id: impl Into<String>,
) -> anyhow::Result<()> {
    let instance_pre = workload.instantiate_pre(component_id).await?;
    let pre = CoreHandlerPre::new(instance_pre)?;
    let workload_id = workload_id.into();

    for sub in subs {
        let conn = conn.clone();
        let in_flight = Arc::new(tokio::sync::Semaphore::new(conn.limits.max_in_flight));
        let workload = workload.clone();
        let component_id = component_id.to_string();
        let pre = pre.clone();
        let cancel_token = cancel_token.clone();
        let fuel_meter = fuel_meter.clone();
        let failure_sink = failure_sink.clone();
        let workload_id = workload_id.clone();

        tokio::spawn(async move {
            // A SUB the server refuses is refused after the fact: the
            // subscription is accepted locally and simply never delivers, so
            // without this the workload runs forever receiving nothing.
            let mut denials = conn.subscription_denials.subscribe();
            let mut denial_reported = false;

            let subscriber = match &sub.queue_group {
                Some(group) => {
                    conn.client
                        .queue_subscribe(sub.subject.clone(), group.clone())
                        .await
                }
                None => conn.client.subscribe(sub.subject.clone()).await,
            };
            let messages = match subscriber {
                Ok(s) => s,
                Err(e) => {
                    warn!("failed to subscribe to core subject '{}': {e}", sub.subject);
                    return;
                }
            };

            // The subscription is read by its own task and this loop takes
            // from the backlog it fills, so that waiting for a handler permit
            // never means leaving the client's channel unread. See
            // `spawn_core_reader` for what that costs when it is not done.
            let backlog = Arc::new(CoreBacklog::default());
            let (deliveries, mut inbound) =
                tokio::sync::mpsc::channel(conn.limits.subscription_capacity);
            spawn_core_reader(
                sub.subject.clone(),
                messages,
                deliveries,
                backlog.clone(),
                conn.limits.subscription_capacity_bytes,
                cancel_token.clone(),
            );

            loop {
                // Taken before a message is, so that waiting for capacity
                // never blinds the loop to cancellation — see the same
                // ordering in the JetStream loop. Unlike there, holding off
                // here costs nothing beyond latency: the reader keeps the
                // subscription drained either way.
                let permit = tokio::select! {
                    acquired = in_flight.clone().acquire_owned() => match acquired {
                        Ok(p) => p,
                        Err(_) => break,
                    },
                    _ = cancel_token.cancelled() => break,
                };

                tokio::select! {
                    maybe_msg = inbound.recv() => {
                        let raw = match maybe_msg {
                            None => break,
                            Some(m) => m,
                        };
                        // Read before `raw` is handed on, and released here
                        // rather than after the handler returns: the byte
                        // budget bounds what is queued *ahead* of the handler
                        // pool, which `max-in-flight` already bounds.
                        backlog
                            .queued_bytes
                            .fetch_sub(raw.length, Ordering::Relaxed);

                        let mut store = tokio::select! {
                            created = workload.new_store(&component_id) => match created {
                                Err(e) => {
                                    warn!("failed to create store for {component_id}: {e}");
                                    continue;
                                }
                                Ok(s) => s,
                            },
                            // Instantiating into a workload that is being torn
                            // down is work nobody will see the result of.
                            _ = cancel_token.cancelled() => break,
                        };
                        prime_fuel(&mut store);
                        let proxy = tokio::select! {
                            instantiated = pre.instantiate(&mut store) => match instantiated {
                                Err(e) => {
                                    warn!("failed to instantiate {component_id}: {e}");
                                    continue;
                                }
                                Ok(p) => p,
                            },
                            _ = cancel_token.cancelled() => break,
                        };

                        let subject_label = raw.subject.to_string();
                        let span = tracing::span!(
                            tracing::Level::INFO,
                            "incoming_nats_core_message",
                            subject = %subject_label,
                        );

                        // A responder answers on the inbox the requester chose,
                        // which no sane grant covers. This is what authorizes
                        // that one reply, for this inbox only.
                        if let Some(reply) = raw.reply.as_deref() {
                            conn.grant_reply(reply);
                        }
                        let fuel_meter = fuel_meter.clone();
                        tokio::spawn(async move {
                            let _permit = permit;
                            let result = fuel_meter.observe(
                                &[
                                    KeyValue::new("plugin", PLUGIN_NATS_ID),
                                    KeyValue::new("subject", subject_label.clone()),
                                ],
                                &mut store,
                                async move |store| {
                                    proxy
                                        .call(store, &raw)
                                        .instrument(span)
                                        .await
                                        .map_err(Into::into)
                                },
                            ).await;
                            // The handler's own error is the useful one; the
                            // outer result only carries traps.
                            match result {
                                Ok(Err(handler_err)) => warn!(
                                    subject = %subject_label,
                                    "core handler returned an error: {handler_err}"
                                ),
                                Err(e) => warn!(
                                    subject = %subject_label,
                                    "core handler trapped: {e}"
                                ),
                                Ok(Ok(())) => {}
                            }
                        });
                    }
                    // The subscription was accepted locally and refused by the
                    // server, so it will never deliver. Nothing the loop can
                    // do fixes that — the workload's credentials have to.
                    denied = denials.recv() => {
                        if let Ok(subject) = denied
                            && subject == sub.subject
                            && !denial_reported
                        {
                            denial_reported = true;
                            let reason = format!(
                                "NATS server denied SUB on '{subject}'; this subscription \
                                 will never deliver"
                            );
                            warn!(subject = %sub.subject, "{reason}");
                            if let Some(sink) = &failure_sink {
                                sink.report(workload_id.clone(), reason);
                            }
                        }
                    }
                    _ = cancel_token.cancelled() => break,
                }
            }
        });
    }

    Ok(())
}

/// Spawn KV watchers. Each watcher dispatches every `entry` into
/// `handle-event(bucket, entry)`.
///
/// Takes the failure sink and workload id the other two loops report denials
/// through, but has nothing to report yet: a KV watch's ordered consumer
/// delivers to a client-side inbox, so a refused SUB names a subject no watch
/// can attribute to itself. They are here so a watcher-side failure has
/// somewhere to go without changing every call site again.
#[allow(clippy::too_many_arguments)]
pub(super) async fn spawn_kv_watches(
    workload: &ResolvedWorkload,
    component_id: &str,
    conn: Arc<ConnHandle>,
    watches: Vec<KvWatchConfig>,
    cancel_token: CancellationToken,
    fuel_meter: FuelConsumptionMeter,
    _failure_sink: Option<crate::plugin::WorkloadFailureSink>,
    _workload_id: impl Into<String>,
) -> anyhow::Result<()> {
    let instance_pre = workload.instantiate_pre(component_id).await?;
    let pre = KvHandlerPre::new(instance_pre)?;

    for watch in watches {
        let conn = conn.clone();
        let in_flight = Arc::new(tokio::sync::Semaphore::new(conn.limits.max_in_flight));
        let workload = workload.clone();
        let component_id = component_id.to_string();
        let pre = pre.clone();
        let cancel_token = cancel_token.clone();
        let fuel_meter = fuel_meter.clone();

        tokio::spawn(async move {
            // A KV watch is an ordered consumer underneath, which recovers
            // from a reconnect on its own and losslessly. The rebuild is kept
            // anyway — as belt and braces for the failures the ordered
            // consumer does not heal, chiefly a bucket deleted and recreated
            // under the watch — and made lossless in the same way: every
            // rebuild resumes at the revision after the last one dispatched,
            // rather than at whatever the bucket holds when it reattaches.
            let mut reconnects = conn.reconnects.subscribe();
            reconnects.mark_unchanged();
            // The last revision handed to the component, and the bucket's
            // creation time — a recreated bucket restarts revisions at 1, so a
            // position held for the old one would sit past the new one's head
            // and the watch would deliver nothing ever again.
            let mut last_revision: Option<u64> = None;
            let mut bucket_created: Option<i128> = None;
            // A bucket that is briefly absent — a server restart, provisioning
            // that has not caught up — comes back within seconds. A bucket that
            // was never created never does, and a flat 2s retry there is a
            // permanent `STREAM.INFO` loop that logs a WARN every 2s for as
            // long as the workload runs, drowning every other line. So the
            // wait doubles to a cap, and only the first failure of a run is a
            // WARN: the repeats say nothing new.
            let mut consecutive_failures: u32 = 0;

            'watch: loop {
                let store_kv = match conn.jetstream.get_key_value(&watch.bucket).await {
                    Ok(s) => s,
                    Err(e) => {
                        let wait = resubscribe_backoff(consecutive_failures);
                        if consecutive_failures == 0 {
                            warn!(
                                bucket = %watch.bucket,
                                retry_in_ms = wait.as_millis() as u64,
                                "kv watch: bucket not available, retrying with backoff: {e}"
                            );
                        } else {
                            debug!(
                                bucket = %watch.bucket,
                                attempts = consecutive_failures + 1,
                                retry_in_ms = wait.as_millis() as u64,
                                "kv watch: bucket still not available: {e}"
                            );
                        }
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        if wait_or_cancel(&cancel_token, wait).await {
                            break 'watch;
                        }
                        continue 'watch;
                    }
                };
                consecutive_failures = 0;

                // Already fetched by `get_key_value`, so this costs no round
                // trip.
                let created = store_kv.stream.cached_info().created.unix_timestamp_nanos();
                if bucket_created.is_some_and(|previous| previous != created) {
                    warn!(
                        bucket = %watch.bucket,
                        "kv bucket was recreated, restarting the watch from its head"
                    );
                    last_revision = None;
                }
                bucket_created = Some(created);

                // With nothing dispatched yet the watch starts from now, which
                // is what the interface promises a fresh watch. After that,
                // resuming at the next revision is what keeps the writes made
                // during an outage from being silently skipped.
                let watching = match last_revision {
                    Some(revision) => {
                        store_kv
                            .watch_from_revision(watch.filter.as_str(), revision.saturating_add(1))
                            .await
                    }
                    None => store_kv.watch(watch.filter.as_str()).await,
                };
                let mut stream = match watching {
                    Ok(s) => s,
                    Err(e) => {
                        let wait = resubscribe_backoff(consecutive_failures);
                        consecutive_failures = consecutive_failures.saturating_add(1);
                        warn!(
                            retry_in_ms = wait.as_millis() as u64,
                            "kv watch: failed to watch '{}:{}': {e}", watch.bucket, watch.filter
                        );
                        if wait_or_cancel(&cancel_token, wait).await {
                            break 'watch;
                        }
                        continue 'watch;
                    }
                };

                loop {
                    // Taken before an entry is, so that waiting for capacity
                    // never blinds the loop to cancellation or a reconnect —
                    // see the same ordering in the JetStream loop.
                    let permit = tokio::select! {
                        acquired = in_flight.clone().acquire_owned() => match acquired {
                            Ok(p) => p,
                            Err(_) => break 'watch,
                        },
                        _ = reconnects.changed() => {
                            warn!(
                                bucket = %watch.bucket,
                                "reconnected to NATS, rebuilding KV watch"
                            );
                            break;
                        }
                        _ = cancel_token.cancelled() => break 'watch,
                    };

                    tokio::select! {
                        maybe = stream.next() => {
                            let entry = match maybe {
                                Some(Ok(e)) => e,
                                // A consumer-level error is this watch's
                                // delivery path dying — a missed heartbeat, a
                                // deleted consumer, a bucket replaced under
                                // us. Swallowing it leaves the watch attached
                                // to nothing, silently, forever; rebuilding
                                // re-anchors it on whatever the bucket is now.
                                Some(Err(e)) => {
                                    if matches!(
                                        e.kind(),
                                        async_nats::jetstream::kv::WatcherErrorKind::Consumer
                                    ) {
                                        warn!(
                                            bucket = %watch.bucket,
                                            "kv watch delivery lost, rebuilding watch: {e}"
                                        );
                                        break;
                                    }
                                    warn!("kv watch stream error: {e}");
                                    continue;
                                }
                                None => break,
                            };

                            let bucket_name = watch.bucket.clone();
                            // Recorded before the entry moves into the handler
                            // task: this is where a rebuild resumes from.
                            last_revision = Some(entry.revision);

                            let mut store = tokio::select! {
                                created = workload.new_store(&component_id) => match created {
                                    Err(e) => {
                                        warn!("failed to create store for {component_id}: {e}");
                                        continue;
                                    }
                                    Ok(s) => s,
                                },
                                // Instantiating into a workload that is being
                                // torn down is work nobody will see the result
                                // of.
                                _ = cancel_token.cancelled() => break 'watch,
                            };
                            prime_fuel(&mut store);
                            let proxy = tokio::select! {
                                instantiated = pre.instantiate(&mut store) => match instantiated {
                                    Err(e) => {
                                        warn!("failed to instantiate {component_id}: {e}");
                                        continue;
                                    }
                                    Ok(p) => p,
                                },
                                _ = cancel_token.cancelled() => break 'watch,
                            };

                            let key_label = entry.key.clone();
                            let span = tracing::span!(
                                tracing::Level::INFO,
                                "incoming_nats_kv_event",
                                bucket = %bucket_name,
                                key = %key_label,
                            );

                            let fuel_meter = fuel_meter.clone();
                            tokio::spawn(async move {
                                let _permit = permit;
                                let bucket_for_label = bucket_name.clone();
                                let result = fuel_meter.observe(
                                    &[
                                        KeyValue::new("plugin", PLUGIN_NATS_ID),
                                        KeyValue::new("bucket", bucket_for_label.clone()),
                                    ],
                                    &mut store,
                                    async move |store| {
                                        proxy
                                            .call(store, &bucket_name, &entry)
                                            .instrument(span)
                                            .await
                                            .map_err(Into::into)
                                    },
                                ).await;
                                match result {
                                    Ok(Err(handler_err)) => warn!(
                                        bucket = %bucket_for_label,
                                        key = %key_label,
                                        "KV handler returned an error: {handler_err}"
                                    ),
                                    Err(e) => warn!(
                                        bucket = %bucket_for_label,
                                        key = %key_label,
                                        "KV handler trapped: {e}"
                                    ),
                                    Ok(Ok(())) => {}
                                }
                            });
                        }
                        _ = reconnects.changed() => {
                            warn!(
                                bucket = %watch.bucket,
                                "reconnected to NATS, rebuilding KV watch"
                            );
                            break;
                        }
                        _ = cancel_token.cancelled() => break 'watch,
                    }
                }

                if wait_or_cancel(&cancel_token, RESUBSCRIBE_BACKOFF).await {
                    break 'watch;
                }
            }
        });
    }

    // Silence unused import warnings; BucketHandle is re-exported in case
    // future watchers want to pass a bucket resource into the handler.
    let _ = std::marker::PhantomData::<BucketHandle>;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering::Relaxed;

    #[test]
    fn a_backlog_under_budget_admits() {
        assert!(admits(0, 512, 4096));
        assert!(admits(3584, 512, 4096));
    }

    #[test]
    fn a_backlog_that_would_exceed_the_budget_sheds() {
        assert!(!admits(3585, 512, 4096));
        assert!(!admits(4096, 1, 4096));
    }

    #[test]
    fn an_empty_backlog_admits_a_message_larger_than_the_whole_budget() {
        // Otherwise a budget set below one payload is not a tight bound, it is
        // a subscription that delivers nothing and never says why.
        assert!(admits(0, 1_048_576, 4096));
    }

    #[test]
    fn shedding_is_counted_exactly() {
        // The property the `SlowConsumer` event cannot offer: every shed
        // delivery lands in the totals, whether or not it was reported. The
        // interval throttles the log line, never the accounting.
        let backlog = CoreBacklog::default();
        let mut last_report = None;
        for _ in 0..10_000 {
            record_shed(&backlog, "fan.work", 64, &mut last_report, "backlog full");
        }
        assert_eq!(backlog.shed.load(Relaxed), 10_000);
        assert_eq!(backlog.shed_bytes.load(Relaxed), 640_000);
    }

    #[test]
    fn shedding_reports_at_most_once_per_interval() {
        let backlog = CoreBacklog::default();
        let mut last_report = None;
        record_shed(&backlog, "fan.work", 64, &mut last_report, "backlog full");
        let first = last_report.expect("the first shed always reports");
        for _ in 0..1_000 {
            record_shed(&backlog, "fan.work", 64, &mut last_report, "backlog full");
        }
        assert_eq!(
            last_report,
            Some(first),
            "a burst inside one interval should not move the report mark"
        );
    }
}
