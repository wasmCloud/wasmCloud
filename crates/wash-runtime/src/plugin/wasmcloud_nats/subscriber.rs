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
    core_bindings, jetstream_bindings, kv_bindings,
};

/// Naks immediately so a failed setup does not stall the consumer for the
/// full ack-wait before the message is redelivered.
async fn nak_now(acker: &async_nats::jetstream::message::Acker) {
    if let Err(e) = acker
        .ack_with(async_nats::jetstream::AckKind::Nak(None))
        .await
    {
        warn!("failed to nak after a delivery setup failure: {e}");
    }
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
pub(super) async fn spawn_jetstream_subscriptions(
    workload: &ResolvedWorkload,
    component_id: &str,
    conn: Arc<ConnHandle>,
    subs: Vec<JetStreamSubscriptionConfig>,
    cancel_token: CancellationToken,
    fuel_meter: FuelConsumptionMeter,
) -> anyhow::Result<()> {
    let instance_pre = workload.instantiate_pre(component_id).await?;
    let pre = jetstream_bindings::NatsJsProcessorPre::new(instance_pre)?;

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
            // Attach only. Creating the stream here would let a subscription
            // provision arbitrary streams outside the grant; the interface
            // contract is that streams are provisioned out-of-band.
            let stream = match conn.jetstream.get_stream(&sub.stream).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(
                        "failed to get/create JetStream stream '{}': {e}",
                        sub.stream
                    );
                    return;
                }
            };

            let deliver_policy = match sub.deliver_policy.as_str() {
                "all" => jetstream::consumer::DeliverPolicy::All,
                "last" => jetstream::consumer::DeliverPolicy::Last,
                "last-per-subject" => jetstream::consumer::DeliverPolicy::LastPerSubject,
                _ => jetstream::consumer::DeliverPolicy::New,
            };

            // With a queue group the consumer must be shared across hosts, or
            // each host creates its own and every host sees every message.
            // Without one it stays ephemeral and per-host.
            let (durable_name, deliver_subject) = match &sub.queue_group {
                Some(group) => (
                    Some(group.clone()),
                    format!("_nats_push.{}.{group}", sub.stream),
                ),
                None => (None, format!("_nats_push.{}", uuid::Uuid::new_v4())),
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
                    return;
                }
            };

            let mut messages = match consumer.messages().await {
                Ok(m) => m,
                Err(e) => {
                    warn!(
                        "failed to get message stream for '{}': {e}",
                        sub.filter_subject
                    );
                    return;
                }
            };

            loop {
                tokio::select! {
                    maybe_msg = messages.next() => {
                        let raw = match maybe_msg {
                            None => break,
                            Some(Err(e)) => {
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

                        // Bound fan-out into the component pool: an unbounded
                        // spawn per message turns a traffic spike into an OOM.
                        let permit = match in_flight.clone().acquire_owned().await {
                            Ok(p) => p,
                            Err(_) => break,
                        };

                        let mut store = match workload.new_store(&component_id).await {
                            Err(e) => {
                                warn!("failed to create store for {component_id}: {e}");
                                nak_now(&acker).await;
                                continue;
                            }
                            Ok(s) => s,
                        };
                        let proxy = match pre.instantiate_async(&mut store).await {
                            Err(e) => {
                                warn!("failed to instantiate {component_id}: {e}");
                                nak_now(&acker).await;
                                continue;
                            }
                            Ok(p) => p,
                        };

                        // Under `auto` the host settles the message, so the
                        // guest handle carries no acker.
                        let (guest_acker, host_acker) = match ack_mode {
                            AckMode::Auto => (None, Some(acker)),
                            AckMode::Manual => (Some(acker), None),
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
                                        nak_now(&acker).await;
                                    }
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
                                        .wasmcloud_nats_jetstream_handler()
                                        .call_handle_message(store, resource)
                                        .instrument(span)
                                        .await
                                        .map_err(Into::into)
                                },
                            ).await;
                            // Under `auto`, the handler's outcome decides the
                            // ack. A trap or an `Err` naks, so the message is
                            // redelivered instead of stalling for ack-wait.
                            if let Some(acker) = host_acker {
                                let settled = match &result {
                                    Ok(Ok(())) => acker.ack().await,
                                    _ => {
                                        acker
                                            .ack_with(async_nats::jetstream::AckKind::Nak(None))
                                            .await
                                    }
                                };
                                if let Err(e) = settled {
                                    warn!("failed to settle JetStream message: {e}");
                                }
                            }
                            if let Err(e) = result {
                                warn!("Error handling JetStream message: {e}");
                            }
                            drop(permit);
                        });
                    }
                    _ = cancel_token.cancelled() => break,
                }
            }
        });
    }

    Ok(())
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
    let pre = core_bindings::NatsSubscriberPre::new(instance_pre)?;

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

                        let reply_to = raw.reply.as_ref().map(|r| r.to_string());
                        let headers = raw.headers.as_ref().map(nats_headers_to_core_wit);
                        let msg = core_bindings::wasmcloud::nats::types::NatsMessage {
                            subject: raw.subject.to_string(),
                            reply_to,
                            body: raw.payload.to_vec(),
                            headers,
                        };

                        let mut store = match workload.new_store(&component_id).await {
                            Err(e) => {
                                warn!("failed to create store for {component_id}: {e}");
                                continue;
                            }
                            Ok(s) => s,
                        };
                        let proxy = match pre.instantiate_async(&mut store).await {
                            Err(e) => {
                                warn!("failed to instantiate {component_id}: {e}");
                                continue;
                            }
                            Ok(p) => p,
                        };

                        let span = tracing::span!(
                            tracing::Level::INFO,
                            "incoming_nats_core_message",
                            subject = %msg.subject,
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
                                    KeyValue::new("subject", msg.subject.clone()),
                                ],
                                &mut store,
                                async move |store| {
                                    proxy
                                        .wasmcloud_nats_core_handler()
                                        .call_handle_message(store, &msg)
                                        .instrument(span)
                                        .await
                                        .map_err(Into::into)
                                },
                            ).await;
                            if let Err(e) = result {
                                warn!("Error handling core message: {e}");
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
    let pre = kv_bindings::NatsKvWatcherPre::new(instance_pre)?;

    for watch in watches {
        let conn = conn.clone();
        let in_flight = Arc::new(tokio::sync::Semaphore::new(conn.limits.max_in_flight));
        let workload = workload.clone();
        let component_id = component_id.to_string();
        let pre = pre.clone();
        let cancel_token = cancel_token.clone();
        let fuel_meter = fuel_meter.clone();

        tokio::spawn(async move {
            let store_kv = match conn.jetstream.get_key_value(&watch.bucket).await {
                Ok(s) => s,
                Err(e) => {
                    warn!("kv watch: bucket '{}' not available: {e}", watch.bucket);
                    return;
                }
            };

            let mut stream = match store_kv.watch(watch.filter.as_str()).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(
                        "kv watch: failed to watch '{}:{}': {e}",
                        watch.bucket, watch.filter
                    );
                    return;
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

                        let wit_entry = kv_entry_to_kv_handler_wit(&entry);
                        let bucket_name = watch.bucket.clone();

                        let mut store = match workload.new_store(&component_id).await {
                            Err(e) => {
                                warn!("failed to create store for {component_id}: {e}");
                                continue;
                            }
                            Ok(s) => s,
                        };
                        let proxy = match pre.instantiate_async(&mut store).await {
                            Err(e) => {
                                warn!("failed to instantiate {component_id}: {e}");
                                continue;
                            }
                            Ok(p) => p,
                        };

                        let span = tracing::span!(
                            tracing::Level::INFO,
                            "incoming_nats_kv_event",
                            bucket = %bucket_name,
                            key = %wit_entry.key,
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
                                    KeyValue::new("bucket", bucket_for_label),
                                ],
                                &mut store,
                                async move |store| {
                                    proxy
                                        .wasmcloud_nats_kv_handler()
                                        .call_handle_event(store, &bucket_name, &wit_entry)
                                        .instrument(span)
                                        .await
                                        .map_err(Into::into)
                                },
                            ).await;
                            if let Err(e) = result {
                                warn!("Error handling KV event: {e}");
                            }
                        });
                    }
                    _ = cancel_token.cancelled() => break,
                }
            }
        });
    }

    // Silence unused import warnings; BucketHandle is re-exported in case
    // future watchers want to pass a bucket resource into the handler.
    let _ = std::marker::PhantomData::<BucketHandle>;
    Ok(())
}
