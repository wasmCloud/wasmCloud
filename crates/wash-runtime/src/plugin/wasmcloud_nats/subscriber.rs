//! Long-lived subscription loops spawned per workload.
//!
//! Three kinds of loops: JetStream push (with explicit ack via `MessageHandle`),
//! core NATS subscriptions (no ack), and KV watches. Each loop dispatches into
//! the component's split `wasmcloud:nats/*-handler` exports.

use std::sync::Arc;

use async_nats::jetstream;
use futures::StreamExt;
use opentelemetry::KeyValue;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, warn};

use crate::engine::workload::ResolvedWorkload;
use crate::observability::FuelConsumptionMeter;
use crate::wasmtime::component::Resource;

use super::config::AckMode;
use super::conn::ConnHandle;
use super::handles::{BucketHandle, MessageHandle};
use super::{
    CoreSubscriptionConfig, JetStreamSubscriptionConfig, KvWatchConfig, PLUGIN_NATS_ID,
    async_core_bindings, async_jetstream_bindings, async_kv_bindings, core_bindings,
    jetstream_bindings, kv_bindings,
};

use crate::engine::ctx::SharedCtx;

/// Which revision of a handler export a component serves.
///
/// A component exports either the sync `@0.1.0` handler or the async `@0.2.0`
/// one; the pre-instantiation is what type-checks that, so the async revision
/// is tried first and the sync one is the fallback.
macro_rules! handler_pre {
    ($name:ident, $proxy:ident, $sync:ty, $async:ty, $sync_proxy:ty, $async_proxy:ty) => {
        enum $name {
            Sync($sync),
            Async($async),
        }

        enum $proxy {
            Sync($sync_proxy),
            Async($async_proxy),
        }

        impl Clone for $name {
            fn clone(&self) -> Self {
                match self {
                    Self::Sync(p) => Self::Sync(p.clone()),
                    Self::Async(p) => Self::Async(p.clone()),
                }
            }
        }

        impl $name {
            fn new(
                instance_pre: crate::wasmtime::component::InstancePre<SharedCtx>,
            ) -> anyhow::Result<Self> {
                match <$async>::new(instance_pre.clone()) {
                    Ok(p) => Ok(Self::Async(p)),
                    Err(async_err) => match <$sync>::new(instance_pre) {
                        Ok(p) => Ok(Self::Sync(p)),
                        Err(sync_err) => Err(anyhow::anyhow!(
                            "component exports neither the async (@0.2.0) nor the sync (@0.1.0) \
                             handler: {async_err}; {sync_err}"
                        )),
                    },
                }
            }

            async fn instantiate(
                &self,
                store: &mut crate::wasmtime::Store<SharedCtx>,
            ) -> anyhow::Result<$proxy> {
                Ok(match self {
                    Self::Sync(p) => $proxy::Sync(p.instantiate_async(store).await?),
                    Self::Async(p) => $proxy::Async(p.instantiate_async(store).await?),
                })
            }
        }
    };
}

handler_pre!(
    JsHandlerPre,
    JsProxy,
    jetstream_bindings::NatsJsProcessorPre<SharedCtx>,
    async_jetstream_bindings::NatsAsyncJsProcessorPre<SharedCtx>,
    jetstream_bindings::NatsJsProcessor,
    async_jetstream_bindings::NatsAsyncJsProcessor
);
handler_pre!(
    CoreHandlerPre,
    CoreProxy,
    core_bindings::NatsSubscriberPre<SharedCtx>,
    async_core_bindings::NatsAsyncSubscriberPre<SharedCtx>,
    core_bindings::NatsSubscriber,
    async_core_bindings::NatsAsyncSubscriber
);
handler_pre!(
    KvHandlerPre,
    KvProxy,
    kv_bindings::NatsKvWatcherPre<SharedCtx>,
    async_kv_bindings::NatsAsyncKvWatcherPre<SharedCtx>,
    kv_bindings::NatsKvWatcher,
    async_kv_bindings::NatsAsyncKvWatcher
);

impl JsProxy {
    async fn call(
        &self,
        store: &mut crate::wasmtime::Store<SharedCtx>,
        handle: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), String>> {
        match self {
            Self::Sync(p) => {
                p.wasmcloud_nats0_1_0_jetstream_handler()
                    .call_handle_message(store, handle)
                    .await
            }
            // An `async func` export binds through the concurrent ABI, so it
            // takes an `Accessor` and must be driven inside `run_concurrent` —
            // which is also what lets the guest await its own imports while the
            // host keeps the store pumping.
            Self::Async(p) => {
                store
                    .run_concurrent(async move |accessor| {
                        p.wasmcloud_nats0_2_0_jetstream_handler()
                            .call_handle_message(accessor, handle)
                            .await
                    })
                    .await?
            }
        }
    }
}

impl CoreProxy {
    /// Builds the message in whichever revision's types the component serves —
    /// the two records are structurally identical but distinct Rust types.
    async fn call(
        &self,
        store: &mut crate::wasmtime::Store<SharedCtx>,
        raw: &async_nats::Message,
    ) -> wasmtime::Result<Result<(), String>> {
        match self {
            Self::Sync(p) => {
                let msg = core_bindings::wasmcloud::nats0_1_0::types::NatsMessage {
                    subject: raw.subject.to_string(),
                    reply_to: raw.reply.as_ref().map(|r| r.to_string()),
                    body: raw.payload.to_vec(),
                    headers: raw.headers.as_ref().map(nats_headers_to_core_wit),
                };
                p.wasmcloud_nats0_1_0_core_handler()
                    .call_handle_message(store, &msg)
                    .await
            }
            Self::Async(p) => {
                let msg = async_core_bindings::wasmcloud::nats0_2_0::types::NatsMessage {
                    subject: raw.subject.to_string(),
                    reply_to: raw.reply.as_ref().map(|r| r.to_string()),
                    body: raw.payload.to_vec(),
                    headers: raw.headers.as_ref().map(nats_headers_to_async_core_wit),
                };
                store
                    .run_concurrent(async move |accessor| {
                        p.wasmcloud_nats0_2_0_core_handler()
                            .call_handle_message(accessor, msg)
                            .await
                    })
                    .await?
            }
        }
    }
}

impl KvProxy {
    async fn call(
        &self,
        store: &mut crate::wasmtime::Store<SharedCtx>,
        bucket: &str,
        entry: &jetstream::kv::Entry,
    ) -> wasmtime::Result<Result<(), String>> {
        match self {
            Self::Sync(p) => {
                p.wasmcloud_nats0_1_0_kv_handler()
                    .call_handle_event(store, bucket, &kv_entry_to_kv_handler_wit(entry))
                    .await
            }
            Self::Async(p) => {
                let (bucket, entry) = (bucket.to_string(), kv_entry_to_async_kv_handler_wit(entry));
                store
                    .run_concurrent(async move |accessor| {
                        p.wasmcloud_nats0_2_0_kv_handler()
                            .call_handle_event(accessor, bucket, entry)
                            .await
                    })
                    .await?
            }
        }
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
const IDLE_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(15);

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

fn nats_headers_to_core_wit(
    headers: &async_nats::HeaderMap,
) -> Vec<core_bindings::wasmcloud::nats0_1_0::types::HeaderEntry> {
    let mut out = Vec::new();
    for (name, values) in headers.iter() {
        for value in values {
            out.push(core_bindings::wasmcloud::nats0_1_0::types::HeaderEntry {
                name: name.to_string(),
                value: value.as_str().to_string(),
            });
        }
    }
    out
}

fn nats_headers_to_async_core_wit(
    headers: &async_nats::HeaderMap,
) -> Vec<async_core_bindings::wasmcloud::nats0_2_0::types::HeaderEntry> {
    let mut out = Vec::new();
    for (name, values) in headers.iter() {
        for value in values {
            out.push(
                async_core_bindings::wasmcloud::nats0_2_0::types::HeaderEntry {
                    name: name.to_string(),
                    value: value.as_str().to_string(),
                },
            );
        }
    }
    out
}

fn kv_entry_to_async_kv_handler_wit(
    e: &jetstream::kv::Entry,
) -> async_kv_bindings::wasmcloud::nats0_2_0::kv::Entry {
    let operation = match e.operation {
        jetstream::kv::Operation::Put => {
            async_kv_bindings::wasmcloud::nats0_2_0::kv::KvOperation::Put
        }
        jetstream::kv::Operation::Delete => {
            async_kv_bindings::wasmcloud::nats0_2_0::kv::KvOperation::Delete
        }
        jetstream::kv::Operation::Purge => {
            async_kv_bindings::wasmcloud::nats0_2_0::kv::KvOperation::Purge
        }
    };

    async_kv_bindings::wasmcloud::nats0_2_0::kv::Entry {
        key: e.key.clone(),
        value: e.value.to_vec(),
        revision: e.revision,
        created_at_unix_nanos: e.created.unix_timestamp_nanos().max(0) as u64,
        operation,
    }
}

fn kv_entry_to_kv_handler_wit(
    e: &jetstream::kv::Entry,
) -> kv_bindings::wasmcloud::nats0_1_0::kv::Entry {
    let operation = match e.operation {
        jetstream::kv::Operation::Put => kv_bindings::wasmcloud::nats0_1_0::kv::KvOperation::Put,
        jetstream::kv::Operation::Delete => {
            kv_bindings::wasmcloud::nats0_1_0::kv::KvOperation::Delete
        }
        jetstream::kv::Operation::Purge => {
            kv_bindings::wasmcloud::nats0_1_0::kv::KvOperation::Purge
        }
    };

    kv_bindings::wasmcloud::nats0_1_0::kv::Entry {
        key: e.key.clone(),
        value: e.value.to_vec(),
        revision: e.revision,
        created_at_unix_nanos: e.created.unix_timestamp_nanos().max(0) as u64,
        operation,
    }
}

/// Spawn a JetStream push subscription per entry. Each consumer uses explicit
/// ack so the handler can decide ack / nak / term via the `message-handle`.
pub(super) async fn spawn_jetstream_subscriptions(
    workload: &ResolvedWorkload,
    component_id: &str,
    conn: Arc<ConnHandle>,
    subs: Vec<JetStreamSubscriptionConfig>,
    cancel_token: CancellationToken,
    fuel_meter: FuelConsumptionMeter,
) -> anyhow::Result<()> {
    let instance_pre = workload.instantiate_pre(component_id).await?;
    let pre = JsHandlerPre::new(instance_pre)?;

    for sub in subs {
        let conn = conn.clone();
        let ack_mode = conn.ack_mode;
        let in_flight = Arc::new(tokio::sync::Semaphore::new(conn.limits.max_in_flight));
        let workload = workload.clone();
        let component_id = component_id.to_string();
        let pre = pre.clone();
        let cancel_token = cancel_token.clone();
        let fuel_meter = fuel_meter.clone();

        tokio::spawn(async move {
            // Mark the current generation as seen, so the connect that opened
            // this connection does not immediately count as a reconnect.
            let mut reconnects = conn.reconnects.subscribe();
            reconnects.mark_unchanged();

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
            let in_flight_sequences: Arc<std::sync::Mutex<std::collections::BTreeSet<u64>>> =
                Arc::new(std::sync::Mutex::new(std::collections::BTreeSet::new()));
            let mut last_delivered: Option<u64> = None;
            // The stream's creation time, so a stream that was deleted and
            // recreated under us is recognised as a different stream: its
            // sequences restart at 1 and our old position means nothing.
            let mut stream_created: Option<i128> = None;
            let mut consecutive_failures = 0u32;

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

                // With a queue group the consumer must be shared across hosts,
                // or each host creates its own and every host sees every
                // message. Without one it stays ephemeral and per-host, and
                // gets a fresh deliver subject on every rebuild.
                let (durable_name, deliver_subject, inactive_threshold) = match &sub.queue_group {
                    Some(group) => (
                        Some(group.clone()),
                        format!("_nats_push.{}.{group}", sub.stream),
                        std::time::Duration::ZERO,
                    ),
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
                    None => in_flight_sequences
                        .lock()
                        .ok()
                        .and_then(|pending| pending.iter().next().copied())
                        .or_else(|| last_delivered.map(|seq| seq.saturating_add(1))),
                };
                let info = stream.cached_info();
                let created = info.created.unix_timestamp_nanos();
                let recreated = stream_created.is_some_and(|previous| previous != created);
                stream_created = Some(created);
                let deliver_policy = match resume_from {
                    // Nothing was ever delivered, so there is no position to
                    // resume from: the configured policy still applies.
                    None => configured_policy,
                    // Same stream, and our position is still within it.
                    Some(start_sequence)
                        if !recreated && start_sequence <= info.state.last_sequence + 1 =>
                    {
                        jetstream::consumer::DeliverPolicy::ByStartSequence { start_sequence }
                    }
                    // The stream was replaced, or rolled out from under us.
                    // Take what it holds rather than skipping past it.
                    Some(_) => jetstream::consumer::DeliverPolicy::ByStartSequence {
                        start_sequence: info.state.first_sequence.max(1),
                    },
                };

                let consumer = match stream
                    .create_consumer(jetstream::consumer::push::Config {
                        durable_name,
                        filter_subject: sub.filter_subject.clone(),
                        deliver_subject,
                        deliver_group: sub.queue_group.clone(),
                        ack_policy: jetstream::consumer::AckPolicy::Explicit,
                        ack_wait: std::time::Duration::from_secs(30),
                        deliver_policy,
                        idle_heartbeat: IDLE_HEARTBEAT,
                        inactive_threshold,
                        ..Default::default()
                    })
                    .await
                {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(
                            "failed to create push consumer for '{}': {e}",
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

                consecutive_failures = 0;

                loop {
                    tokio::select! {
                        maybe_msg = messages.next() => {
                            let raw = match maybe_msg {
                                None => break,
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

                            let (sequence, delivery_count) = match raw.info() {
                                Ok(i) => (i.stream_sequence, i.delivered as u32),
                                Err(_) => (0, 1),
                            };
                            let subject_str = raw.subject.to_string();
                            let (message, acker) = raw.split();

                            last_delivered =
                                Some(last_delivered.map_or(sequence, |seen| seen.max(sequence)));
                            if let Ok(mut pending) = in_flight_sequences.lock() {
                                pending.insert(sequence);
                            }
                            let settled_sequences = in_flight_sequences.clone();

                            // Bound fan-out into the component pool: an unbounded
                            // spawn per message turns a traffic spike into an OOM.
                            let permit = match in_flight.clone().acquire_owned().await {
                                Ok(p) => p,
                                Err(_) => break,
                            };

                            let mut store = match workload.new_store(&component_id).await {
                                Err(e) => {
                                    warn!("failed to create store for {component_id}: {e}");
                                    nak_with_backoff(&acker).await;
                                    release_sequence(&settled_sequences, sequence);
                                    continue;
                                }
                                Ok(s) => s,
                            };
                            prime_fuel(&mut store);
                            let proxy = match pre.instantiate(&mut store).await {
                                Err(e) => {
                                    warn!("failed to instantiate {component_id}: {e}");
                                    nak_with_backoff(&acker).await;
                                    release_sequence(&settled_sequences, sequence);
                                    continue;
                                }
                                Ok(p) => p,
                            };

                            // Under `auto` the host settles the message, so the
                            // guest handle carries no acker.
                            let (guest_acker, host_acker) = match ack_mode {
                                AckMode::Auto => (None, Some(acker)),
                                AckMode::Manual => (Some(std::sync::Arc::new(acker)), None),
                            };
                            let handle = MessageHandle {
                                acker: guest_acker,
                                message,
                                sequence,
                                delivery_count,
                            };
                            let resource: Resource<MessageHandle> =
                                match store.data_mut().table.push(handle) {
                                    Ok(r) => r,
                                    Err(e) => {
                                        warn!("failed to push message-handle for {component_id}: {e}");
                                        if let Some(acker) = host_acker {
                                            nak_with_backoff(&acker).await;
                                        }
                                        release_sequence(&settled_sequences, sequence);
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

                                // Under `auto`, the handler's outcome decides the
                                // ack. A trap or an `Err` naks so the message is
                                // redelivered, with a backoff that grows by
                                // delivery count: a handler that fails permanently
                                // would otherwise spin as fast as the server can
                                // redeliver.
                                if let Some(acker) = host_acker {
                                    let settled = match &result {
                                        Ok(Ok(())) => acker.ack().await,
                                        _ => {
                                            acker
                                                .ack_with(async_nats::jetstream::AckKind::Nak(Some(
                                                    redelivery_backoff(delivery_count),
                                                )))
                                                .await
                                        }
                                    };
                                    if let Err(e) = settled {
                                        warn!("failed to settle JetStream message: {e}");
                                    }
                                }
                                release_sequence(&settled_sequences, sequence);
                                drop(permit);
                            });
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

                if wait_or_cancel(&cancel_token, RESUBSCRIBE_BACKOFF).await {
                    break 'delivery;
                }
            }
        });
    }

    Ok(())
}

/// Drops a sequence from the in-flight set, so a rebuild does not rewind to it.
fn release_sequence(
    sequences: &Arc<std::sync::Mutex<std::collections::BTreeSet<u64>>>,
    sequence: u64,
) {
    if let Ok(mut pending) = sequences.lock() {
        pending.remove(&sequence);
    }
}

/// Sleeps for `delay`, returning true if the workload was cancelled instead.
async fn wait_or_cancel(cancel_token: &CancellationToken, delay: std::time::Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        _ = cancel_token.cancelled() => true,
    }
}

/// Spawn core NATS subscribers (no ack semantics, optional queue group).
pub(super) async fn spawn_core_subscriptions(
    workload: &ResolvedWorkload,
    component_id: &str,
    conn: Arc<ConnHandle>,
    subs: Vec<CoreSubscriptionConfig>,
    cancel_token: CancellationToken,
    fuel_meter: FuelConsumptionMeter,
) -> anyhow::Result<()> {
    let instance_pre = workload.instantiate_pre(component_id).await?;
    let pre = CoreHandlerPre::new(instance_pre)?;

    for sub in subs {
        let conn = conn.clone();
        let in_flight = Arc::new(tokio::sync::Semaphore::new(conn.limits.max_in_flight));
        let workload = workload.clone();
        let component_id = component_id.to_string();
        let pre = pre.clone();
        let cancel_token = cancel_token.clone();
        let fuel_meter = fuel_meter.clone();

        tokio::spawn(async move {
            let subscriber = match &sub.queue_group {
                Some(group) => {
                    conn.client
                        .queue_subscribe(sub.subject.clone(), group.clone())
                        .await
                }
                None => conn.client.subscribe(sub.subject.clone()).await,
            };
            let mut messages = match subscriber {
                Ok(s) => s,
                Err(e) => {
                    warn!("failed to subscribe to core subject '{}': {e}", sub.subject);
                    return;
                }
            };

            loop {
                tokio::select! {
                    maybe_msg = messages.next() => {
                        let raw = match maybe_msg {
                            None => break,
                            Some(m) => m,
                        };

                        let mut store = match workload.new_store(&component_id).await {
                            Err(e) => {
                                warn!("failed to create store for {component_id}: {e}");
                                continue;
                            }
                            Ok(s) => s,
                        };
                        prime_fuel(&mut store);
                        let proxy = match pre.instantiate(&mut store).await {
                            Err(e) => {
                                warn!("failed to instantiate {component_id}: {e}");
                                continue;
                            }
                            Ok(p) => p,
                        };

                        let subject_label = raw.subject.to_string();
                        let span = tracing::span!(
                            tracing::Level::INFO,
                            "incoming_nats_core_message",
                            subject = %subject_label,
                        );

                        let permit = match in_flight.clone().acquire_owned().await {
                            Ok(p) => p,
                            Err(_) => break,
                        };
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
                    _ = cancel_token.cancelled() => break,
                }
            }
        });
    }

    Ok(())
}

/// Spawn KV watchers. Each watcher dispatches every `entry` into
/// `handle-event(bucket, entry)`.
pub(super) async fn spawn_kv_watches(
    workload: &ResolvedWorkload,
    component_id: &str,
    conn: Arc<ConnHandle>,
    watches: Vec<KvWatchConfig>,
    cancel_token: CancellationToken,
    fuel_meter: FuelConsumptionMeter,
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
            // A KV watch is a JetStream consumer underneath, so it needs the
            // same rebuild-on-reconnect treatment as a push subscription.
            let mut reconnects = conn.reconnects.subscribe();
            reconnects.mark_unchanged();

            'watch: loop {
                let store_kv = match conn.jetstream.get_key_value(&watch.bucket).await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("kv watch: bucket '{}' not available: {e}", watch.bucket);
                        if wait_or_cancel(&cancel_token, RESUBSCRIBE_BACKOFF).await {
                            break 'watch;
                        }
                        continue 'watch;
                    }
                };

                let mut stream = match store_kv.watch(watch.filter.as_str()).await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(
                            "kv watch: failed to watch '{}:{}': {e}",
                            watch.bucket, watch.filter
                        );
                        if wait_or_cancel(&cancel_token, RESUBSCRIBE_BACKOFF).await {
                            break 'watch;
                        }
                        continue 'watch;
                    }
                };

                loop {
                    tokio::select! {
                        maybe = stream.next() => {
                            let entry = match maybe {
                                Some(Ok(e)) => e,
                                Some(Err(e)) => {
                                    warn!("kv watch stream error: {e}");
                                    continue;
                                }
                                None => break,
                            };

                            let bucket_name = watch.bucket.clone();

                            let mut store = match workload.new_store(&component_id).await {
                                Err(e) => {
                                    warn!("failed to create store for {component_id}: {e}");
                                    continue;
                                }
                                Ok(s) => s,
                            };
                            prime_fuel(&mut store);
                            let proxy = match pre.instantiate(&mut store).await {
                                Err(e) => {
                                    warn!("failed to instantiate {component_id}: {e}");
                                    continue;
                                }
                                Ok(p) => p,
                            };

                            let key_label = entry.key.clone();
                            let span = tracing::span!(
                                tracing::Level::INFO,
                                "incoming_nats_kv_event",
                                bucket = %bucket_name,
                                key = %key_label,
                            );

                            let permit = match in_flight.clone().acquire_owned().await {
                                Ok(p) => p,
                                Err(_) => break,
                            };
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
