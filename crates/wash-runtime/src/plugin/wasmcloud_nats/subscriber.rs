//! Long-lived subscription loops spawned per workload.
//!
//! Three kinds of loops: JetStream push (with explicit ack via `MessageHandle`),
//! core NATS subscriptions (no ack), and KV watches. Each loop dispatches into
//! the component's `wasmcloud:nats/handler` export.

use std::sync::Arc;

use async_nats::jetstream;
use futures::StreamExt;
use opentelemetry::KeyValue;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, warn};

use crate::engine::workload::ResolvedWorkload;
use crate::observability::FuelConsumptionMeter;
use crate::wasmtime::component::Resource;

use super::bindings::{self, wasmcloud::nats::types};
use super::handles::{BucketHandle, MessageHandle};
use super::interfaces::kv_entry_to_wit;
use super::{CoreSubscriptionConfig, JetStreamSubscriptionConfig, KvWatchConfig, PLUGIN_NATS_ID};

/// Spawn a JetStream push subscription per entry. Each consumer uses explicit
/// ack so the handler can decide ack / nak / term via the `message-handle`.
pub(super) async fn spawn_jetstream_subscriptions(
    workload: &ResolvedWorkload,
    component_id: &str,
    jetstream: Arc<jetstream::Context>,
    subs: Vec<JetStreamSubscriptionConfig>,
    cancel_token: CancellationToken,
    fuel_meter: FuelConsumptionMeter,
) -> anyhow::Result<()> {
    let instance_pre = workload.instantiate_pre(component_id).await?;
    let pre = bindings::NatsPre::new(instance_pre)?;

    for sub in subs {
        let jetstream = jetstream.clone();
        let workload = workload.clone();
        let component_id = component_id.to_string();
        let pre = pre.clone();
        let cancel_token = cancel_token.clone();
        let fuel_meter = fuel_meter.clone();

        tokio::spawn(async move {
            let stream = match jetstream
                .get_or_create_stream(jetstream::stream::Config {
                    name: sub.stream.clone(),
                    subjects: vec![sub.filter_subject.clone()],
                    storage: jetstream::stream::StorageType::File,
                    ..Default::default()
                })
                .await
            {
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

            let consumer = match stream
                .create_consumer(jetstream::consumer::push::Config {
                    filter_subject: sub.filter_subject.clone(),
                    deliver_subject: format!("_nats_push.{}", uuid::Uuid::new_v4()),
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
                            None | Some(Err(_)) => break,
                            Some(Ok(m)) => m,
                        };

                        let info = raw.info();
                        let (sequence, delivery_count) = match info.as_ref() {
                            Ok(i) => (i.stream_sequence, i.delivered as u32),
                            Err(_) => (0, 1),
                        };
                        let subject_str = raw.subject.to_string();
                        let body = raw.payload.to_vec();
                        let reply_to = raw.reply.as_ref().map(|r| r.to_string());
                        let headers = raw.headers.clone();

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

                        let handle = MessageHandle {
                            inner: Some(raw),
                            sequence,
                            delivery_count,
                            subject: subject_str.clone(),
                            reply_to,
                            body,
                            headers,
                        };
                        let resource: Resource<MessageHandle> =
                            match store.data_mut().table.push(handle) {
                                Ok(r) => r,
                                Err(e) => {
                                    warn!("failed to push message-handle for {component_id}: {e}");
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
                            if let Err(e) = result {
                                warn!("Error handling JetStream message: {e}");
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

/// Spawn core NATS subscribers (no ack semantics, optional queue group).
pub(super) async fn spawn_core_subscriptions(
    workload: &ResolvedWorkload,
    component_id: &str,
    client: Arc<async_nats::Client>,
    subs: Vec<CoreSubscriptionConfig>,
    cancel_token: CancellationToken,
    fuel_meter: FuelConsumptionMeter,
) -> anyhow::Result<()> {
    let instance_pre = workload.instantiate_pre(component_id).await?;
    let pre = bindings::NatsPre::new(instance_pre)?;

    for sub in subs {
        let client = client.clone();
        let workload = workload.clone();
        let component_id = component_id.to_string();
        let pre = pre.clone();
        let cancel_token = cancel_token.clone();
        let fuel_meter = fuel_meter.clone();

        tokio::spawn(async move {
            let subscriber = match &sub.queue_group {
                Some(group) => {
                    client
                        .queue_subscribe(sub.subject.clone(), group.clone())
                        .await
                }
                None => client.subscribe(sub.subject.clone()).await,
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
                        let headers = raw.headers.as_ref().map(
                            super::interfaces::nats_headers_to_wit,
                        );
                        let msg = types::NatsMessage {
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

                        let fuel_meter = fuel_meter.clone();
                        tokio::spawn(async move {
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
/// `handle-kv-event(bucket, entry)`.
pub(super) async fn spawn_kv_watches(
    workload: &ResolvedWorkload,
    component_id: &str,
    jetstream: Arc<jetstream::Context>,
    watches: Vec<KvWatchConfig>,
    cancel_token: CancellationToken,
    fuel_meter: FuelConsumptionMeter,
) -> anyhow::Result<()> {
    let instance_pre = workload.instantiate_pre(component_id).await?;
    let pre = bindings::NatsPre::new(instance_pre)?;

    for watch in watches {
        let jetstream = jetstream.clone();
        let workload = workload.clone();
        let component_id = component_id.to_string();
        let pre = pre.clone();
        let cancel_token = cancel_token.clone();
        let fuel_meter = fuel_meter.clone();

        tokio::spawn(async move {
            let store_kv = match jetstream.get_key_value(&watch.bucket).await {
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

                        let wit_entry = kv_entry_to_wit(&entry);
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

                        let fuel_meter = fuel_meter.clone();
                        tokio::spawn(async move {
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
