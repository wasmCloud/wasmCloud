use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use bytes::Bytes;
use futures::StreamExt;
use tracing::{instrument, warn};
use wasmtime::component::Resource;

use crate::engine::ctx::ActiveCtx;

use super::bindings::wasmcloud::nats0_1_0::{core, jetstream as js, kv, types};
use super::conn::ConnHandle;
use super::handles::{
    BucketHandle, MessageHandle, PullConsumerHandle, already_settled, jetstream_err,
};
use super::{PLUGIN_NATS_ID, WasmcloudNats};

/// Converts guest headers, rejecting anything async-nats would assert on.
///
/// `HeaderName`/`HeaderValue`'s `From<&str>` impls panic on CRLF and on
/// non-graphic-ASCII names, and guest input is untrusted, so the fallible
/// `FromStr` path is the only safe one here.
pub(super) fn wit_headers_to_nats(
    headers: &[types::HeaderEntry],
) -> Result<async_nats::HeaderMap, types::NatsError> {
    use std::str::FromStr as _;

    let mut map = async_nats::HeaderMap::new();
    for h in headers {
        let name = async_nats::HeaderName::from_str(&h.name).map_err(|e| {
            types::NatsError::Unexpected(format!("invalid header name `{}`: {e}", h.name))
        })?;
        let value = async_nats::HeaderValue::from_str(&h.value).map_err(|e| {
            types::NatsError::Unexpected(format!("invalid value for header `{}`: {e}", h.name))
        })?;
        map.append(name, value);
    }
    Ok(map)
}

/// Serialized size of a header block, mirroring what async-nats puts on the
/// wire. Its own `wire_len` is crate-private.
fn headers_wire_len(headers: &async_nats::HeaderMap) -> usize {
    let mut len = b"NATS/1.0\r\n".len() + b"\r\n".len();
    for (name, values) in headers.iter() {
        for value in values {
            len += name.to_string().len() + b": ".len() + value.as_str().len() + b"\r\n".len();
        }
    }
    len
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

/// Reads the plugin and the calling workload's id off the context.
///
/// Capability calls carry no binding identity, so the workload id is what
/// selects this tenant's client and grant. Kept synchronous so the context
/// borrow is never held across an await — `ActiveCtx` is not `Send`.
fn conn_args<'a>(ctx: &ActiveCtx<'a>) -> Result<(Arc<WasmcloudNats>, Arc<str>), types::NatsError> {
    let plugin = ctx
        .try_get_plugin::<WasmcloudNats>(PLUGIN_NATS_ID)
        .map_err(|e| types::NatsError::Unexpected(format!("nats plugin not available: {e}")))?;
    Ok((plugin, ctx.workload_id.clone()))
}

/// Resolves the calling workload's connection, or fails the call.
macro_rules! conn_or_return {
    ($ctx:expr) => {
        match conn_args($ctx) {
            Ok((plugin, workload_id)) => match plugin.conn_for(&workload_id).await {
                Some(conn) => conn,
                None => {
                    tracing::warn!(%workload_id, "no NATS connection bound for workload");
                    return Ok(Err(types::NatsError::Disconnected));
                }
            },
            Err(e) => return Ok(Err(e)),
        }
    };
}

/// Upper bound on messages one `scan` may buffer into host memory.
const MAX_SCAN_MESSAGES: usize = 1_000;
/// Wall-clock bound on one `scan`, so a slow stream cannot pin the call open.
const MAX_SCAN_DURATION: Duration = Duration::from_secs(10);

/// Maps a policy denial onto the wire error.
fn denied(subject: &str) -> types::NatsError {
    types::NatsError::SubjectDenied(subject.to_string())
}

/// Rejects an oversized payload before it reaches the connection.
///
/// The server counts the serialized header block against `max_payload`, so a
/// body-only check lets header-heavy messages near the cap fail deeper as a
/// transport error instead of the typed variant.
fn check_payload(
    body_len: usize,
    headers: Option<&async_nats::HeaderMap>,
    conn: &ConnHandle,
) -> Result<(), types::NatsError> {
    let size = match headers {
        Some(h) if !h.is_empty() => body_len.saturating_add(headers_wire_len(h)),
        _ => body_len,
    };
    if conn.max_payload > 0 && size as u64 > conn.max_payload {
        return Err(types::NatsError::MaxPayloadExceeded(conn.max_payload));
    }
    Ok(())
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
        let conn = conn_or_return!(self);

        let subject = msg.subject;
        if conn.policy.check_subject(&subject).is_err() {
            return Ok(Err(denied(&subject)));
        }
        if let Some(reply_to) = msg.reply_to.as_deref()
            && conn.policy.check_subject(reply_to).is_err()
        {
            return Ok(Err(denied(reply_to)));
        }
        let headers = match msg.headers.as_deref().filter(|h| !h.is_empty()) {
            Some(h) => match wit_headers_to_nats(h) {
                Ok(map) => Some(map),
                Err(e) => return Ok(Err(e)),
            },
            None => None,
        };
        if let Err(e) = check_payload(msg.body.len(), headers.as_ref(), &conn) {
            return Ok(Err(e));
        }
        let payload: Bytes = msg.body.into();

        let result = match (msg.reply_to, headers) {
            (Some(reply_to), Some(headers)) => {
                conn.client
                    .publish_with_reply_and_headers(subject, reply_to, headers, payload)
                    .await
            }
            (Some(reply_to), None) => {
                conn.client
                    .publish_with_reply(subject, reply_to, payload)
                    .await
            }
            (None, Some(headers)) => {
                conn.client
                    .publish_with_headers(subject, headers, payload)
                    .await
            }
            (None, None) => conn.client.publish(subject, payload).await,
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
        let conn = conn_or_return!(self);

        let types::NatsMessage {
            subject,
            body,
            headers,
            ..
        } = msg;
        if conn.policy.check_subject(&subject).is_err() {
            return Ok(Err(denied(&subject)));
        }
        let headers = match headers.as_deref().filter(|h| !h.is_empty()) {
            Some(h) => match wit_headers_to_nats(h) {
                Ok(map) => Some(map),
                Err(e) => return Ok(Err(e)),
            },
            None => None,
        };
        if let Err(e) = check_payload(body.len(), headers.as_ref(), &conn) {
            return Ok(Err(e));
        }

        let timeout_duration = Duration::from_millis(timeout_ms as u64);
        let request_future = async {
            match headers {
                Some(headers) => {
                    conn.client
                        .request_with_headers(subject, headers, body.into())
                        .await
                }
                None => conn.client.request(subject, body.into()).await,
            }
        };

        let resp = match tokio::time::timeout(timeout_duration, request_future).await {
            Ok(Ok(m)) => m,
            Ok(Err(e)) => {
                // The client's own `request-timeout-ms` fires below the guest
                // timeout, and is still a timeout rather than a transport fault.
                return Ok(Err(match e.kind() {
                    async_nats::RequestErrorKind::NoResponders => types::NatsError::NoResponders,
                    async_nats::RequestErrorKind::TimedOut => {
                        warn!("request timed out in the client: {e}");
                        types::NatsError::Timeout(format!("request timed out: {e}"))
                    }
                    async_nats::RequestErrorKind::MaxPayloadExceeded => {
                        types::NatsError::MaxPayloadExceeded(conn.max_payload)
                    }
                    _ => {
                        warn!("failed to send request: {e}");
                        types::NatsError::Connection(format!("failed to send request: {e}"))
                    }
                }));
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
        let conn = conn_or_return!(self);

        if conn.policy.check_subject(&msg.subject).is_err() {
            return Ok(Err(denied(&msg.subject)));
        }
        let header_map = match msg.headers.as_deref().filter(|h| !h.is_empty()) {
            Some(h) => match wit_headers_to_nats(h) {
                Ok(map) => Some(map),
                Err(e) => return Ok(Err(e)),
            },
            None => None,
        };
        if let Err(e) = check_payload(msg.body.len(), header_map.as_ref(), &conn) {
            return Ok(Err(e));
        }

        let ack_future = if let Some(header_map) = header_map {
            match conn
                .jetstream
                .publish_with_headers(msg.subject, header_map, msg.body.into())
                .await
            {
                Ok(f) => f,
                Err(e) => return Ok(Err(jetstream_err("failed to publish", e))),
            }
        } else {
            match conn.jetstream.publish(msg.subject, msg.body.into()).await {
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
        let conn = conn_or_return!(self);

        if conn.policy.check_stream(&stream_name).is_err() {
            return Ok(Err(denied(&stream_name)));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "stream '{stream_name}': {e}"
                ))));
            }
        };

        if !conn.server_at_least(2, 9) {
            return Ok(Err(types::NatsError::UnsupportedByServer(
                "direct get requires NATS server 2.9 or newer".to_string(),
            )));
        }

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
        let conn = conn_or_return!(self);

        if conn.policy.check_stream(&stream_name).is_err() {
            return Ok(Err(denied(&stream_name)));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
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
        let limit = (max_count as usize).min(MAX_SCAN_MESSAGES);
        let deadline = tokio::time::Instant::now() + MAX_SCAN_DURATION;

        loop {
            if messages.len() >= limit || tokio::time::Instant::now() >= deadline {
                break;
            }
            match tokio::time::timeout(Duration::from_millis(100), msg_stream.next()).await {
                Ok(Some(Ok(msg))) => {
                    // A malformed reply is the server's problem, not the
                    // guest's — report it rather than trapping the instance.
                    let info = match msg.info() {
                        Ok(i) => i,
                        Err(e) => {
                            return Ok(Err(jetstream_err("failed to get message info", e)));
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

        // The consumer is ephemeral to this call; without this each scan leaves
        // one behind on the stream.
        drop(msg_stream);
        if let Err(e) = stream
            .delete_consumer(&pull_consumer.cached_info().name)
            .await
        {
            warn!("failed to clean up scan consumer: {e}");
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
        let conn = conn_or_return!(self);

        if conn.policy.check_stream(&stream_name).is_err() {
            return Ok(Err(denied(&stream_name)));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "stream '{stream_name}': {e}"
                ))));
            }
        };

        // Attach only: `create_consumer` is create-or-update and would rewrite
        // an existing durable's filter subject and deliver policy.
        let opened_consumer = match stream.get_consumer(&consumer).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "consumer '{consumer}' on stream '{stream_name}': {e}"
                ))));
            }
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
            h.message.subject.as_ref(),
            &h.message.payload,
            h.message.reply.as_deref(),
            h.message.headers.as_ref(),
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
        let Some(acker) = h.acker.take() else {
            return Ok(Err(already_settled()));
        };
        match acker.ack().await {
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
        let Some(acker) = h.acker.take() else {
            return Ok(Err(already_settled()));
        };
        let kind = jetstream::AckKind::Nak(delay_ms.map(|ms| Duration::from_millis(ms as u64)));
        match acker.ack_with(kind).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(jetstream_err("nak failed", e))),
        }
    }

    async fn in_progress(
        &mut self,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let h = self.table.get(&rep)?;
        let Some(acker) = h.acker.as_ref() else {
            return Ok(Err(already_settled()));
        };
        match acker.ack_with(jetstream::AckKind::Progress).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(jetstream_err("in-progress failed", e))),
        }
    }

    async fn term(
        &mut self,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let h = self.table.get_mut(&rep)?;
        let Some(acker) = h.acker.take() else {
            return Ok(Err(already_settled()));
        };
        match acker.ack_with(jetstream::AckKind::Term).await {
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
                    let (sequence, delivery_count) = match msg.info() {
                        Ok(info) => (info.stream_sequence, info.delivered as u32),
                        Err(_) => (0, 1),
                    };
                    // Pull consumers are always guest-driven, so the acker
                    // goes to the handle regardless of ack-mode.
                    let (message, acker) = msg.split();
                    handles.push(self.table.push(MessageHandle {
                        acker: Some(Arc::new(acker)),
                        message,
                        sequence,
                        delivery_count,
                    })?);
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
        let conn = conn_or_return!(self);
        if conn.policy.check_bucket(&bucket).is_err() {
            return Ok(Err(denied(&bucket)));
        }

        let store = match conn.jetstream.get_key_value(&bucket).await {
            Ok(store) => store,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "bucket '{bucket}': {e}"
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
        let h = self.table.get_mut(&rep)?;
        // `Store::status` reports the stream info cached when the bucket was
        // opened, so writes made through this same handle read back as zero.
        let info = match h.store.stream.info().await {
            Ok(info) => info.clone(),
            Err(e) => return Ok(Err(jetstream_err("kv status failed", e))),
        };
        Ok(Ok(kv::BucketStatus {
            bucket: h.store.name.clone(),
            values: info.state.messages,
            history: info
                .config
                .max_messages_per_subject
                .clamp(0, u8::MAX as i64) as u8,
            ttl_seconds: info.config.max_age.as_secs(),
            bytes: info.state.bytes,
        }))
    }

    async fn drop(&mut self, rep: Resource<BucketHandle>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(name: &str, value: &str) -> types::HeaderEntry {
        types::HeaderEntry {
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn valid_headers_convert() {
        let map = wit_headers_to_nats(&[header("Nats-Msg-Id", "abc-1")]).unwrap();
        assert_eq!(map.get("Nats-Msg-Id").map(|v| v.as_str()), Some("abc-1"));
    }

    #[test]
    fn guest_headers_cannot_panic_the_host() {
        // async-nats' `From<&str>` conversions assert on these; guest input is
        // untrusted, so they have to come back as a typed error instead.
        for bad in [
            header("X-Ok", "line1\r\nInjected: yes"),
            header("X-Ok", "trailing\n"),
            header("Bad Name", "v"),
            header("Bad:Name", "v"),
            header("Bad\u{7f}Name", "v"),
            header("nøn-ascii", "v"),
        ] {
            let err = wit_headers_to_nats(std::slice::from_ref(&bad))
                .expect_err("expected `{bad:?}` to be refused");
            assert!(
                matches!(err, types::NatsError::Unexpected(_)),
                "unexpected error for {bad:?}: {err:?}"
            );
        }
    }

    #[test]
    fn header_bytes_count_toward_max_payload() {
        let headers = wit_headers_to_nats(&[header("X-Pad", "0123456789")]).unwrap();
        // "NATS/1.0\r\n" + "X-Pad: 0123456789\r\n" + "\r\n"
        assert_eq!(headers_wire_len(&headers), 10 + 19 + 2);
    }
}
