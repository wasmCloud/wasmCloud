use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use bytes::Bytes;
use futures::StreamExt;
use tracing::{instrument, warn};
use wasmtime::component::Resource;

use crate::engine::ctx::ActiveCtx;

use super::bindings::wasmcloud::nats::{core, jetstream as js, kv, types};
use super::handles::{BucketHandle, MessageHandle, PullConsumerHandle, jetstream_err};
use super::{PLUGIN_NATS_ID, WasmcloudNats};

pub(super) fn wit_headers_to_nats(headers: &[types::HeaderEntry]) -> async_nats::HeaderMap {
    let mut map = async_nats::HeaderMap::new();
    for h in headers {
        map.append(h.name.as_str(), h.value.as_str());
    }
    map
}

pub(super) fn nats_headers_to_wit(headers: &async_nats::HeaderMap) -> Vec<types::HeaderEntry> {
    let mut out = Vec::new();
    for (name, values) in headers.iter() {
        for value in values {
            out.push(types::HeaderEntry {
                name: name.to_string(),
                value: value.as_str().to_string(),
            });
        }
    }
    out
}

fn build_nats_message(
    subject: &str,
    body: &[u8],
    reply_to: Option<&str>,
    headers: Option<&async_nats::HeaderMap>,
) -> types::NatsMessage {
    types::NatsMessage {
        subject: subject.to_string(),
        reply_to: reply_to.map(|s| s.to_string()),
        body: body.to_vec(),
        headers: headers.map(nats_headers_to_wit),
    }
}

fn plugin_for<'a>(ctx: &ActiveCtx<'a>) -> Result<Arc<WasmcloudNats>, types::NatsError> {
    ctx.get_plugin::<WasmcloudNats>(PLUGIN_NATS_ID)
        .ok_or_else(|| types::NatsError::Unexpected("nats plugin not available".to_string()))
}

fn kv_op_to_wit(op: jetstream::kv::Operation) -> kv::KvOperation {
    match op {
        jetstream::kv::Operation::Put => kv::KvOperation::Put,
        jetstream::kv::Operation::Delete => kv::KvOperation::Delete,
        jetstream::kv::Operation::Purge => kv::KvOperation::Purge,
    }
}

pub(super) fn kv_entry_to_wit(e: &jetstream::kv::Entry) -> kv::Entry {
    kv::Entry {
        key: e.key.clone(),
        value: e.value.to_vec(),
        revision: e.revision,
        created_at_unix_nanos: e.created.unix_timestamp_nanos().max(0) as u64,
        operation: kv_op_to_wit(e.operation),
    }
}

impl<'a> types::Host for ActiveCtx<'a> {}

impl<'a> core::Host for ActiveCtx<'a> {
    #[instrument(skip_all, fields(subject = %msg.subject))]
    async fn publish(
        &mut self,
        msg: types::NatsMessage,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let plugin = match plugin_for(self) {
            Ok(p) => p,
            Err(e) => return Ok(Err(e)),
        };

        let subject = msg.subject;
        let payload: Bytes = msg.body.into();
        let has_headers = msg.headers.as_ref().is_some_and(|h| !h.is_empty());

        let result = match (msg.reply_to, has_headers) {
            (Some(reply_to), true) => {
                let headers = wit_headers_to_nats(msg.headers.as_deref().unwrap_or_default());
                plugin
                    .client
                    .publish_with_reply_and_headers(subject, reply_to, headers, payload)
                    .await
            }
            (Some(reply_to), false) => {
                plugin
                    .client
                    .publish_with_reply(subject, reply_to, payload)
                    .await
            }
            (None, true) => {
                let headers = wit_headers_to_nats(msg.headers.as_deref().unwrap_or_default());
                plugin
                    .client
                    .publish_with_headers(subject, headers, payload)
                    .await
            }
            (None, false) => plugin.client.publish(subject, payload).await,
        };

        match result {
            Ok(_) => Ok(Ok(())),
            Err(e) => Ok(Err(types::NatsError::Connection(format!(
                "failed to publish: {e}"
            )))),
        }
    }

    #[instrument(skip_all, fields(subject = %msg.subject, timeout_ms))]
    async fn request(
        &mut self,
        msg: types::NatsMessage,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<types::NatsMessage, types::NatsError>> {
        let plugin = match plugin_for(self) {
            Ok(p) => p,
            Err(e) => return Ok(Err(e)),
        };

        let types::NatsMessage { subject, body, .. } = msg;

        let timeout_duration = Duration::from_millis(timeout_ms as u64);
        let request_future = plugin.client.request(subject, body.into());

        let resp = match tokio::time::timeout(timeout_duration, request_future).await {
            Ok(Ok(m)) => m,
            Ok(Err(e)) => {
                warn!("failed to send request: {e}");
                return Ok(Err(types::NatsError::Connection(format!(
                    "failed to send request: {e}"
                ))));
            }
            Err(_) => {
                warn!("request timed out after {timeout_ms}ms");
                return Ok(Err(types::NatsError::Timeout(format!(
                    "request timed out after {timeout_ms}ms"
                ))));
            }
        };

        Ok(Ok(build_nats_message(
            resp.subject.as_ref(),
            &resp.payload,
            resp.reply.as_deref(),
            resp.headers.as_ref(),
        )))
    }
}

impl<'a> js::Host for ActiveCtx<'a> {
    #[instrument(skip_all, fields(subject = %msg.subject))]
    async fn publish(
        &mut self,
        msg: types::NatsMessage,
    ) -> wasmtime::Result<Result<js::PublishAck, types::NatsError>> {
        let plugin = match plugin_for(self) {
            Ok(p) => p,
            Err(e) => return Ok(Err(e)),
        };

        let ack_future = if let Some(headers) = msg.headers.as_ref().filter(|h| !h.is_empty()) {
            let header_map = wit_headers_to_nats(headers);
            match plugin
                .jetstream
                .publish_with_headers(msg.subject, header_map, msg.body.into())
                .await
            {
                Ok(f) => f,
                Err(e) => return Ok(Err(jetstream_err("failed to publish", e))),
            }
        } else {
            match plugin.jetstream.publish(msg.subject, msg.body.into()).await {
                Ok(f) => f,
                Err(e) => return Ok(Err(jetstream_err("failed to publish", e))),
            }
        };

        let ack = match ack_future.await {
            Ok(a) => a,
            Err(e) => return Ok(Err(jetstream_err("failed to confirm publish", e))),
        };

        Ok(Ok(js::PublishAck {
            stream_name: ack.stream,
            sequence: ack.sequence,
            duplicate: ack.duplicate,
        }))
    }

    #[instrument(skip_all, fields(stream = %stream_name, sequence))]
    async fn get_by_sequence(
        &mut self,
        stream_name: String,
        sequence: u64,
    ) -> wasmtime::Result<Result<js::StoredMessage, types::NatsError>> {
        let plugin = match plugin_for(self) {
            Ok(p) => p,
            Err(e) => return Ok(Err(e)),
        };

        let stream = match plugin.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "stream '{stream_name}': {e}"
                ))));
            }
        };

        match stream.direct_get(sequence).await {
            Ok(m) => Ok(Ok(js::StoredMessage {
                subject: m.subject.to_string(),
                sequence: m.sequence,
                data: m.payload.to_vec(),
                headers: Some(nats_headers_to_wit(&m.headers)),
            })),
            Err(e) => Ok(Err(jetstream_err("get-by-sequence failed", e))),
        }
    }

    #[instrument(skip_all, fields(stream = %stream_name, start_sequence, max_count))]
    async fn scan(
        &mut self,
        stream_name: String,
        start_sequence: u64,
        max_count: u32,
    ) -> wasmtime::Result<Result<Vec<js::StoredMessage>, types::NatsError>> {
        let plugin = match plugin_for(self) {
            Ok(p) => p,
            Err(e) => return Ok(Err(e)),
        };

        let stream = match plugin.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "stream '{stream_name}': {e}"
                ))));
            }
        };

        let effective_start = if start_sequence == 0 {
            1
        } else {
            start_sequence
        };

        let pull_consumer = match stream
            .create_consumer(jetstream::consumer::pull::Config {
                deliver_policy: jetstream::consumer::DeliverPolicy::ByStartSequence {
                    start_sequence: effective_start,
                },
                ..Default::default()
            })
            .await
        {
            Ok(c) => c,
            Err(e) => return Ok(Err(jetstream_err("failed to create scan consumer", e))),
        };

        let mut msg_stream = match pull_consumer.messages().await {
            Ok(m) => m,
            Err(e) => return Ok(Err(jetstream_err("failed to get messages", e))),
        };

        let mut messages = Vec::new();
        let limit = max_count as usize;

        loop {
            if messages.len() >= limit {
                break;
            }
            match tokio::time::timeout(Duration::from_millis(100), msg_stream.next()).await {
                Ok(Some(Ok(msg))) => {
                    let info = match msg.info() {
                        Ok(i) => i,
                        Err(e) => {
                            return Err(wasmtime::Error::msg(format!(
                                "failed to get message info: {e}"
                            )));
                        }
                    };
                    messages.push(js::StoredMessage {
                        subject: msg.subject.to_string(),
                        sequence: info.stream_sequence,
                        data: msg.payload.to_vec(),
                        headers: msg.headers.as_ref().map(nats_headers_to_wit),
                    });
                }
                Ok(Some(Err(e))) => {
                    warn!("error reading message: {e}");
                    break;
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }

        messages.sort_by_key(|m| m.sequence);
        Ok(Ok(messages))
    }

    #[instrument(skip_all, fields(stream = %stream_name, consumer = %consumer))]
    async fn open_pull_consumer(
        &mut self,
        stream_name: String,
        consumer: String,
    ) -> wasmtime::Result<Result<Resource<PullConsumerHandle>, types::NatsError>> {
        let plugin = match plugin_for(self) {
            Ok(p) => p,
            Err(e) => return Ok(Err(e)),
        };

        let stream = match plugin.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "stream '{stream_name}': {e}"
                ))));
            }
        };

        let consumer_cfg = jetstream::consumer::pull::Config {
            name: Some(consumer.clone()),
            durable_name: Some(consumer.clone()),
            ..Default::default()
        };

        let opened_consumer = match stream.create_consumer(consumer_cfg).await {
            Ok(c) => c,
            Err(e) => return Ok(Err(jetstream_err("open pull-consumer failed", e))),
        };

        let handle = PullConsumerHandle {
            consumer: Some(opened_consumer),
        };
        let resource = self.table.push(handle)?;
        Ok(Ok(resource))
    }
}

impl<'a> js::HostMessageHandle for ActiveCtx<'a> {
    async fn message(
        &mut self,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<types::NatsMessage> {
        let h = self.table.get(&rep)?;
        Ok(build_nats_message(
            &h.subject,
            &h.body,
            h.reply_to.as_deref(),
            h.headers.as_ref(),
        ))
    }

    async fn sequence(&mut self, rep: Resource<MessageHandle>) -> wasmtime::Result<u64> {
        let h = self.table.get(&rep)?;
        Ok(h.sequence)
    }

    async fn delivery_count(&mut self, rep: Resource<MessageHandle>) -> wasmtime::Result<u32> {
        let h = self.table.get(&rep)?;
        Ok(h.delivery_count)
    }

    async fn ack(
        &mut self,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let h = self.table.get_mut(&rep)?;
        let Some(msg) = h.inner.take() else {
            return Ok(Err(types::NatsError::Unexpected(
                "message already acked/nak'd/term'd".to_string(),
            )));
        };
        match msg.ack().await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(jetstream_err("ack failed", e))),
        }
    }

    async fn nak(
        &mut self,
        rep: Resource<MessageHandle>,
        delay_ms: Option<u32>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let h = self.table.get_mut(&rep)?;
        let Some(msg) = h.inner.take() else {
            return Ok(Err(types::NatsError::Unexpected(
                "message already acked/nak'd/term'd".to_string(),
            )));
        };
        let kind = jetstream::AckKind::Nak(delay_ms.map(|ms| Duration::from_millis(ms as u64)));
        match msg.ack_with(kind).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(jetstream_err("nak failed", e))),
        }
    }

    async fn in_progress(
        &mut self,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let h = self.table.get(&rep)?;
        let Some(msg) = h.inner.as_ref() else {
            return Ok(Err(types::NatsError::Unexpected(
                "message already acked/nak'd/term'd".to_string(),
            )));
        };
        match msg.ack_with(jetstream::AckKind::Progress).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(jetstream_err("in-progress failed", e))),
        }
    }

    async fn term(
        &mut self,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let h = self.table.get_mut(&rep)?;
        let Some(msg) = h.inner.take() else {
            return Ok(Err(types::NatsError::Unexpected(
                "message already acked/nak'd/term'd".to_string(),
            )));
        };
        match msg.ack_with(jetstream::AckKind::Term).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(jetstream_err("term failed", e))),
        }
    }

    async fn drop(&mut self, rep: Resource<MessageHandle>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl<'a> js::HostPullConsumer for ActiveCtx<'a> {
    async fn fetch(
        &mut self,
        rep: Resource<PullConsumerHandle>,
        batch: u32,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<Vec<Resource<MessageHandle>>, types::NatsError>> {
        let consumer = {
            let handle = self.table.get(&rep)?;
            match handle.consumer.as_ref() {
                Some(consumer) => consumer.clone(),
                None => {
                    return Ok(Err(types::NatsError::Unexpected(
                        "pull consumer has been dropped".to_string(),
                    )));
                }
            }
        };

        let fetch = consumer
            .fetch()
            .max_messages(batch as usize)
            .expires(Duration::from_millis(timeout_ms as u64));
        let mut stream = match fetch.messages().await {
            Ok(s) => s,
            Err(e) => return Ok(Err(jetstream_err("fetch failed", e))),
        };

        let mut handles = Vec::new();
        while let Some(next) = stream.next().await {
            match next {
                Ok(msg) => {
                    let info_res = msg.info();
                    let (sequence, delivery_count) = match info_res {
                        Ok(info) => (info.stream_sequence, info.delivered as u32),
                        Err(_) => (0, 1),
                    };
                    let subject = msg.subject.to_string();
                    let body = msg.payload.to_vec();
                    let reply_to = msg.reply.as_ref().map(|r| r.to_string());
                    let headers = msg.headers.clone();
                    let handle = MessageHandle {
                        inner: Some(msg),
                        sequence,
                        delivery_count,
                        subject,
                        reply_to,
                        body,
                        headers,
                    };
                    handles.push(self.table.push(handle)?);
                }
                Err(e) => {
                    warn!("pull-consumer fetch stream error: {e}");
                    break;
                }
            }
        }

        if handles.is_empty() {
            return Ok(Err(types::NatsError::NoMessages));
        }
        Ok(Ok(handles))
    }

    async fn drop(&mut self, rep: Resource<PullConsumerHandle>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

impl<'a> kv::Host for ActiveCtx<'a> {
    #[instrument(skip_all, fields(bucket = %bucket))]
    async fn open(
        &mut self,
        bucket: String,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, types::NatsError>> {
        let plugin = match plugin_for(self) {
            Ok(p) => p,
            Err(e) => return Ok(Err(e)),
        };
        let store = match plugin.get_kv(&bucket).await {
            Some(s) => s,
            None => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "bucket '{bucket}'"
                ))));
            }
        };
        let resource = self.table.push(BucketHandle { store })?;
        Ok(Ok(resource))
    }
}

// ──────────────────────────────────────────────────────────────────────────
// kv::HostBucket
// ──────────────────────────────────────────────────────────────────────────

const KV_KEYS_BATCH: usize = 1000;

impl<'a> kv::HostBucket for ActiveCtx<'a> {
    async fn get(
        &mut self,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<Option<kv::Entry>, types::NatsError>> {
        let h = self.table.get(&rep)?;
        match h.store.entry(&key).await {
            Ok(Some(e)) => Ok(Ok(Some(kv_entry_to_wit(&e)))),
            Ok(None) => Ok(Ok(None)),
            Err(e) => Ok(Err(jetstream_err("kv get failed", e))),
        }
    }

    async fn put(
        &mut self,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        let h = self.table.get(&rep)?;
        match h.store.put(&key, value.into()).await {
            Ok(rev) => Ok(Ok(rev)),
            Err(e) => Ok(Err(jetstream_err("kv put failed", e))),
        }
    }

    async fn create(
        &mut self,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        let h = self.table.get(&rep)?;
        match h.store.create(&key, value.into()).await {
            Ok(rev) => Ok(Ok(rev)),
            Err(e) => Ok(Err(jetstream_err("kv create failed", e))),
        }
    }

    async fn update(
        &mut self,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
        expected_revision: u64,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        let h = self.table.get(&rep)?;
        match h.store.update(&key, value.into(), expected_revision).await {
            Ok(rev) => Ok(Ok(rev)),
            Err(e) => {
                let msg = e.to_string();
                if msg.to_ascii_lowercase().contains("wrong last sequence") {
                    let actual = h
                        .store
                        .entry(&key)
                        .await
                        .ok()
                        .flatten()
                        .map(|e| e.revision)
                        .unwrap_or(0);
                    return Ok(Err(types::NatsError::RevisionMismatch(actual)));
                }
                Ok(Err(jetstream_err("kv update failed", e)))
            }
        }
    }

    async fn delete(
        &mut self,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let h = self.table.get(&rep)?;
        match h.store.delete(&key).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(jetstream_err("kv delete failed", e))),
        }
    }

    async fn purge(
        &mut self,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let h = self.table.get(&rep)?;
        match h.store.purge(&key).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(jetstream_err("kv purge failed", e))),
        }
    }

    async fn keys(
        &mut self,
        rep: Resource<BucketHandle>,
    ) -> wasmtime::Result<Result<Vec<String>, types::NatsError>> {
        let h = self.table.get(&rep)?;
        let mut iter = match h.store.keys().await {
            Ok(i) => i,
            Err(e) => return Ok(Err(jetstream_err("kv keys failed", e))),
        };
        let mut out = Vec::new();
        while let Some(next) = iter.next().await {
            match next {
                Ok(k) => {
                    out.push(k);
                    if out.len() >= KV_KEYS_BATCH {
                        warn!("kv keys truncated at {KV_KEYS_BATCH} entries — bucket has more");
                        break;
                    }
                }
                Err(e) => return Ok(Err(jetstream_err("kv keys iter failed", e))),
            }
        }
        Ok(Ok(out))
    }

    async fn history(
        &mut self,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<Vec<kv::Entry>, types::NatsError>> {
        let h = self.table.get(&rep)?;
        let mut hist = match h.store.history(&key).await {
            Ok(h) => h,
            Err(e) => return Ok(Err(jetstream_err("kv history failed", e))),
        };
        let mut out = Vec::new();
        while let Some(next) = hist.next().await {
            match next {
                Ok(e) => out.push(kv_entry_to_wit(&e)),
                Err(e) => return Ok(Err(jetstream_err("kv history iter failed", e))),
            }
        }
        Ok(Ok(out))
    }

    async fn status(
        &mut self,
        rep: Resource<BucketHandle>,
    ) -> wasmtime::Result<Result<kv::BucketStatus, types::NatsError>> {
        let h = self.table.get(&rep)?;
        match h.store.status().await {
            Ok(s) => {
                let ttl = s.max_age();
                Ok(Ok(kv::BucketStatus {
                    bucket: s.bucket().to_string(),
                    values: s.values(),
                    history: s.history().max(0) as u8,
                    ttl_seconds: ttl.as_secs(),
                    bytes: s.info.state.bytes,
                }))
            }
            Err(e) => Ok(Err(jetstream_err("kv status failed", e))),
        }
    }

    async fn drop(&mut self, rep: Resource<BucketHandle>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}
