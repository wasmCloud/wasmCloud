//! # Async `wasmcloud:nats@0.2.0` (WASI P3)
//!
//! The async-native counterpart of [`super::interfaces`]. Every function in
//! `@0.2.0` is an `async func`, so a component targeting WASI P3 can bind NATS
//! at all: lifting a sync-signature function with the async canonical ABI fails
//! component validation, which is what made `@0.1.0` unusable from a P3 guest.
//!
//! Because the WIT functions are `async func`s the generated host traits use
//! wasmtime's *concurrent* ABI: methods are `async fn`s on `SharedCtx` taking
//! an [`Accessor`], rather than `&mut self` methods on `ActiveCtx`. For a guest
//! that means a `request` no longer blocks the instance, and a handler can
//! await a KV read while the host keeps delivering.
//!
//! Both revisions run off one [`ConnHandle`] per workload: the same connection,
//! grant, and limits back the sync and async surfaces.

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use bytes::Bytes;
use futures::StreamExt;
use tracing::warn;
use wasmtime::component::{Accessor, Resource};

use crate::engine::ctx::{ActiveCtx, SharedCtx};

use super::conn::ConnHandle;
use super::handles::{BucketHandle, MessageHandle, PullConsumerHandle};
use super::policy::Denied;
use super::{PLUGIN_NATS_ID, WasmcloudNats};

pub(super) mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "nats-async-imports",
        imports: { default: async | trappable | tracing },
        with: {
            "wasmcloud:nats/jetstream@0.2.0.message-handle": super::super::handles::MessageHandle,
            "wasmcloud:nats/jetstream@0.2.0.pull-consumer": super::super::handles::PullConsumerHandle,
            "wasmcloud:nats/kv@0.2.0.bucket": super::super::handles::BucketHandle,
        },
    });
}

use bindings::wasmcloud::nats0_2_0::{core, jetstream as js, kv, types};

/// Upper bound on messages one `scan` may buffer into host memory.
const MAX_SCAN_MESSAGES: usize = 1_000;
/// Wall-clock bound on one `scan`, so a slow stream cannot pin the call open.
const MAX_SCAN_DURATION: Duration = Duration::from_secs(10);
/// Cap on keys returned by one `keys` call.
const KV_KEYS_BATCH: usize = 1000;

// ──────────────────────────────────────────────────────────────────────────
// Shared helpers
// ──────────────────────────────────────────────────────────────────────────

/// Resolves the calling workload's connection.
///
/// The context borrow is released before the await: `ActiveCtx` is not `Send`.
async fn conn<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
) -> Result<Arc<ConnHandle>, types::NatsError> {
    let (plugin, workload_id) = accessor.with(|mut a| {
        let ctx = a.get();
        (
            ctx.try_get_plugin::<WasmcloudNats>(PLUGIN_NATS_ID),
            ctx.workload_id.clone(),
        )
    });
    let plugin = plugin
        .map_err(|e| types::NatsError::Unexpected(format!("nats plugin not available: {e}")))?;
    match plugin.conn_for(&workload_id).await {
        Some(conn) => Ok(conn),
        None => {
            warn!(%workload_id, "no NATS connection bound for workload");
            Err(types::NatsError::Disconnected)
        }
    }
}

macro_rules! conn_or_return {
    ($accessor:expr) => {
        match conn($accessor).await {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        }
    };
}

/// Lowers a policy refusal onto the wire, keeping the reason and the kind of
/// name that was refused — `@0.1.0` collapsed both into one string.
fn denied(reason: Denied, target: types::DeniedResource, name: &str) -> types::NatsError {
    types::NatsError::Denied(types::Denial {
        reason: match reason {
            Denied::Reserved => types::DenialReason::Reserved,
            Denied::NotGranted => types::DenialReason::NotGranted,
            Denied::WildcardNotAllowed => types::DenialReason::WildcardNotAllowed,
        },
        target,
        name: name.to_string(),
    })
}

/// Checks a publish/request subject, lowering a refusal.
fn check_subject(conn: &ConnHandle, subject: &str) -> Result<(), types::NatsError> {
    conn.policy
        .check_subject(subject)
        .map_err(|reason| denied(reason, types::DeniedResource::Subject, subject))
}

fn check_stream(conn: &ConnHandle, stream: &str) -> Result<(), types::NatsError> {
    conn.policy
        .check_stream(stream)
        .map_err(|reason| denied(reason, types::DeniedResource::Stream, stream))
}

fn check_bucket(conn: &ConnHandle, bucket: &str) -> Result<(), types::NatsError> {
    conn.policy
        .check_bucket(bucket)
        .map_err(|reason| denied(reason, types::DeniedResource::Bucket, bucket))
}

fn jetstream_err(ctx: impl std::fmt::Display, e: impl std::fmt::Display) -> types::NatsError {
    types::NatsError::Jetstream(format!("{ctx}: {e}"))
}

/// Reported when a guest settles a message the host already owns.
fn already_settled() -> types::NatsError {
    types::NatsError::Unexpected(
        "message already settled, or acknowledgement is owned by the host under ack-mode: auto"
            .to_string(),
    )
}

/// Converts guest headers, rejecting anything async-nats would assert on.
fn wit_headers_to_nats(
    headers: &[types::HeaderEntry],
) -> Result<async_nats::HeaderMap, types::NatsError> {
    use std::str::FromStr as _;

    let mut map = async_nats::HeaderMap::new();
    for h in headers {
        let name = async_nats::HeaderName::from_str(&h.name).map_err(|e| {
            types::NatsError::InvalidHeader(format!("invalid header name `{}`: {e}", h.name))
        })?;
        let value = async_nats::HeaderValue::from_str(&h.value).map_err(|e| {
            types::NatsError::InvalidHeader(format!("invalid value for header `{}`: {e}", h.name))
        })?;
        map.append(name, value);
    }
    Ok(map)
}

/// Builds the header map for an outbound message, or `None` when there is none.
fn outbound_headers(
    headers: Option<&[types::HeaderEntry]>,
) -> Result<Option<async_nats::HeaderMap>, types::NatsError> {
    match headers.filter(|h| !h.is_empty()) {
        Some(h) => wit_headers_to_nats(h).map(Some),
        None => Ok(None),
    }
}

fn nats_headers_to_wit(headers: &async_nats::HeaderMap) -> Vec<types::HeaderEntry> {
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

/// Rejects an oversized payload before it reaches the connection. The server
/// counts the header block against `max_payload`, so this does too.
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

fn kv_entry_to_wit(e: &jetstream::kv::Entry) -> kv::Entry {
    kv::Entry {
        key: e.key.clone(),
        value: e.value.to_vec(),
        revision: e.revision,
        created_at_unix_nanos: e.created.unix_timestamp_nanos().max(0) as u64,
        operation: match e.operation {
            jetstream::kv::Operation::Put => kv::KvOperation::Put,
            jetstream::kv::Operation::Delete => kv::KvOperation::Delete,
            jetstream::kv::Operation::Purge => kv::KvOperation::Purge,
        },
    }
}

fn consumer_info_to_wit(info: &jetstream::consumer::Info) -> js::ConsumerInfo {
    js::ConsumerInfo {
        name: info.name.clone(),
        stream_name: info.stream_name.clone(),
        filter_subject: info.config.filter_subject.clone(),
        max_ack_pending: info.config.max_ack_pending.max(0) as u64,
        max_waiting: info.config.max_waiting.max(0) as u64,
        max_deliver: info.config.max_deliver.max(0) as u64,
        ack_wait_ms: info.config.ack_wait.as_millis().min(u64::MAX as u128) as u64,
        num_ack_pending: info.num_ack_pending as u64,
        num_pending: info.num_pending,
        num_redelivered: info.num_redelivered as u64,
    }
}

/// Clones the consumer out of its resource, so the table borrow is not held
/// across an await — `ActiveCtx` is not `Send`.
fn consumer_ref<T>(
    access: &mut wasmtime::component::Access<'_, T, SharedCtx>,
    rep: &Resource<PullConsumerHandle>,
) -> wasmtime::Result<Option<jetstream::consumer::Consumer<jetstream::consumer::pull::Config>>> {
    Ok(access.get().table.get(rep)?.consumer.clone())
}

/// Clones the bucket's store out of its resource, for the same reason.
fn store_ref<T>(
    access: &mut wasmtime::component::Access<'_, T, SharedCtx>,
    rep: &Resource<BucketHandle>,
) -> wasmtime::Result<jetstream::kv::Store> {
    Ok(access.get().table.get(rep)?.store.clone())
}

// ──────────────────────────────────────────────────────────────────────────
// types / core
// ──────────────────────────────────────────────────────────────────────────

impl types::Host for ActiveCtx<'_> {}

impl<T: 'static + Send> core::HostWithStore<T> for SharedCtx {
    async fn publish(
        accessor: &Accessor<T, Self>,
        msg: types::NatsMessage,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let conn = conn_or_return!(accessor);

        let types::NatsMessage {
            subject,
            body,
            reply_to,
            headers,
        } = msg;
        if let Err(e) = check_subject(&conn, &subject) {
            return Ok(Err(e));
        }
        if let Some(reply_to) = reply_to.as_deref()
            && let Err(e) = check_subject(&conn, reply_to)
        {
            return Ok(Err(e));
        }
        let headers = match outbound_headers(headers.as_deref()) {
            Ok(h) => h,
            Err(e) => return Ok(Err(e)),
        };
        if let Err(e) = check_payload(body.len(), headers.as_ref(), &conn) {
            return Ok(Err(e));
        }

        let payload: Bytes = body.into();
        let result = match (reply_to, headers) {
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

        Ok(result.map_err(|e| types::NatsError::Connection(format!("failed to publish: {e}"))))
    }

    async fn request(
        accessor: &Accessor<T, Self>,
        msg: types::NatsMessage,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<types::NatsMessage, types::NatsError>> {
        let conn = conn_or_return!(accessor);

        let types::NatsMessage {
            subject,
            body,
            headers,
            ..
        } = msg;
        if let Err(e) = check_subject(&conn, &subject) {
            return Ok(Err(e));
        }
        let headers = match outbound_headers(headers.as_deref()) {
            Ok(h) => h,
            Err(e) => return Ok(Err(e)),
        };
        if let Err(e) = check_payload(body.len(), headers.as_ref(), &conn) {
            return Ok(Err(e));
        }

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

        let resp =
            match tokio::time::timeout(Duration::from_millis(timeout_ms as u64), request_future)
                .await
            {
                Ok(Ok(m)) => m,
                Ok(Err(e)) => {
                    // The client's own `request-timeout-ms` can fire below the
                    // guest timeout, and is still a timeout, not a transport fault.
                    return Ok(Err(match e.kind() {
                        async_nats::RequestErrorKind::NoResponders => {
                            types::NatsError::NoResponders
                        }
                        async_nats::RequestErrorKind::TimedOut => {
                            types::NatsError::Timeout(format!("request timed out: {e}"))
                        }
                        async_nats::RequestErrorKind::MaxPayloadExceeded => {
                            types::NatsError::MaxPayloadExceeded(conn.max_payload)
                        }
                        _ => types::NatsError::Connection(format!("failed to send request: {e}")),
                    }));
                }
                Err(_) => {
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

impl core::Host for ActiveCtx<'_> {}

// ──────────────────────────────────────────────────────────────────────────
// jetstream
// ──────────────────────────────────────────────────────────────────────────

impl<T: 'static + Send> js::HostWithStore<T> for SharedCtx {
    async fn publish(
        accessor: &Accessor<T, Self>,
        msg: types::NatsMessage,
    ) -> wasmtime::Result<Result<js::PublishAck, types::NatsError>> {
        let conn = conn_or_return!(accessor);

        if let Err(e) = check_subject(&conn, &msg.subject) {
            return Ok(Err(e));
        }
        let headers = match outbound_headers(msg.headers.as_deref()) {
            Ok(h) => h,
            Err(e) => return Ok(Err(e)),
        };
        if let Err(e) = check_payload(msg.body.len(), headers.as_ref(), &conn) {
            return Ok(Err(e));
        }

        let ack_future = match headers {
            Some(headers) => {
                conn.jetstream
                    .publish_with_headers(msg.subject, headers, msg.body.into())
                    .await
            }
            None => conn.jetstream.publish(msg.subject, msg.body.into()).await,
        };
        let ack_future = match ack_future {
            Ok(f) => f,
            Err(e) => return Ok(Err(jetstream_err("failed to publish", e))),
        };

        match ack_future.await {
            Ok(ack) => Ok(Ok(js::PublishAck {
                stream_name: ack.stream,
                sequence: ack.sequence,
                duplicate: ack.duplicate,
            })),
            Err(e) => Ok(Err(jetstream_err("failed to confirm publish", e))),
        }
    }

    async fn get_by_sequence(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        sequence: u64,
    ) -> wasmtime::Result<Result<js::StoredMessage, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }
        if !conn.server_at_least(2, 9) {
            return Ok(Err(types::NatsError::UnsupportedByServer(
                "direct get requires NATS server 2.9 or newer".to_string(),
            )));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
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

    async fn scan(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        start_sequence: u64,
        max_count: u32,
    ) -> wasmtime::Result<Result<Vec<js::StoredMessage>, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "stream '{stream_name}': {e}"
                ))));
            }
        };

        let effective_start = start_sequence.max(1);
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

        while messages.len() < limit && tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), msg_stream.next()).await {
                Ok(Some(Ok(msg))) => {
                    let sequence = match msg.info() {
                        Ok(i) => i.stream_sequence,
                        Err(e) => {
                            return Ok(Err(jetstream_err("failed to get message info", e)));
                        }
                    };
                    messages.push(js::StoredMessage {
                        subject: msg.subject.to_string(),
                        sequence,
                        data: msg.payload.to_vec(),
                        headers: msg.headers.as_ref().map(nats_headers_to_wit),
                    });
                }
                Ok(Some(Err(e))) => {
                    warn!("error reading message: {e}");
                    break;
                }
                Ok(None) | Err(_) => break,
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

    async fn open_pull_consumer(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        consumer: String,
    ) -> wasmtime::Result<Result<Resource<PullConsumerHandle>, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
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
        let opened = match stream.get_consumer(&consumer).await {
            Ok(c) => c,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "consumer '{consumer}' on stream '{stream_name}': {e}"
                ))));
            }
        };

        let resource = accessor.with(|mut a| {
            a.get().table.push(PullConsumerHandle {
                consumer: Some(opened),
            })
        })?;
        Ok(Ok(resource))
    }

    async fn get_stream_info(
        accessor: &Accessor<T, Self>,
        stream_name: String,
    ) -> wasmtime::Result<Result<js::StreamInfo, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "stream '{stream_name}': {e}"
                ))));
            }
        };
        let info = stream.cached_info();
        Ok(Ok(js::StreamInfo {
            name: info.config.name.clone(),
            subjects: info.config.subjects.clone(),
            messages: info.state.messages,
            bytes: info.state.bytes,
            first_sequence: info.state.first_sequence,
            last_sequence: info.state.last_sequence,
            consumer_count: info.state.consumer_count as u64,
        }))
    }

    async fn list_stream_subjects(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        subject_filter: String,
    ) -> wasmtime::Result<Result<Vec<js::SubjectCount>, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "stream '{stream_name}': {e}"
                ))));
            }
        };

        let filter = if subject_filter.is_empty() {
            ">".to_string()
        } else {
            subject_filter
        };
        let mut info = match stream.info_with_subjects(&filter).await {
            Ok(i) => i,
            Err(e) => return Ok(Err(jetstream_err("stream info failed", e))),
        };

        let mut out = Vec::new();
        while let Some(next) = info.next().await {
            match next {
                Ok((subject, count)) => out.push(js::SubjectCount {
                    subject,
                    count: count as u64,
                }),
                Err(e) => return Ok(Err(jetstream_err("stream subjects failed", e))),
            }
        }
        out.sort_by(|a, b| a.subject.cmp(&b.subject));
        Ok(Ok(out))
    }

    async fn get_consumer_info(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        consumer: String,
    ) -> wasmtime::Result<Result<js::ConsumerInfo, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "stream '{stream_name}': {e}"
                ))));
            }
        };
        match stream.consumer_info(&consumer).await {
            Ok(info) => Ok(Ok(consumer_info_to_wit(&info))),
            Err(e) => Ok(Err(types::NatsError::NotFound(format!(
                "consumer '{consumer}' on stream '{stream_name}': {e}"
            )))),
        }
    }
}

impl js::Host for ActiveCtx<'_> {}

// ──────────────────────────────────────────────────────────────────────────
// jetstream.message-handle
// ──────────────────────────────────────────────────────────────────────────

/// Removes the handle's acker, making settling one-shot.
fn take_acker<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
    rep: &Resource<MessageHandle>,
) -> wasmtime::Result<Option<Arc<jetstream::message::Acker>>> {
    accessor.with(|mut a| Ok(a.get().table.get_mut(rep)?.acker.take()))
}

/// Settles a message with `kind`, consuming the handle's acker.
async fn settle<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
    rep: Resource<MessageHandle>,
    kind: jetstream::AckKind,
    what: &'static str,
) -> wasmtime::Result<Result<(), types::NatsError>> {
    let Some(acker) = take_acker(accessor, &rep)? else {
        return Ok(Err(already_settled()));
    };
    Ok(acker
        .ack_with(kind)
        .await
        .map_err(|e| jetstream_err(format!("{what} failed"), e)))
}

impl<T: 'static + Send> js::HostMessageHandleWithStore<T> for SharedCtx {
    async fn ack(
        accessor: &Accessor<T, Self>,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let Some(acker) = take_acker(accessor, &rep)? else {
            return Ok(Err(already_settled()));
        };
        Ok(acker
            .ack()
            .await
            .map_err(|e| jetstream_err("ack failed", e)))
    }

    async fn ack_sync(
        accessor: &Accessor<T, Self>,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let Some(acker) = take_acker(accessor, &rep)? else {
            return Ok(Err(already_settled()));
        };
        Ok(acker
            .double_ack()
            .await
            .map_err(|e| jetstream_err("ack-sync failed", e)))
    }

    async fn nak(
        accessor: &Accessor<T, Self>,
        rep: Resource<MessageHandle>,
        delay_ms: Option<u32>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let kind = jetstream::AckKind::Nak(delay_ms.map(|ms| Duration::from_millis(ms as u64)));
        settle(accessor, rep, kind, "nak").await
    }

    async fn term(
        accessor: &Accessor<T, Self>,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        settle(accessor, rep, jetstream::AckKind::Term, "term").await
    }

    /// Unlike ack/nak/term this leaves the acker in place — ack-wait can be
    /// extended as often as the handler needs. Cloning the `Arc` rather than
    /// taking it keeps a concurrent `ack` on the same handle working.
    async fn in_progress(
        accessor: &Accessor<T, Self>,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let acker = accessor
            .with(|mut a| -> wasmtime::Result<_> { Ok(a.get().table.get(&rep)?.acker.clone()) })?;
        let Some(acker) = acker else {
            return Ok(Err(already_settled()));
        };
        Ok(acker
            .ack_with(jetstream::AckKind::Progress)
            .await
            .map_err(|e| jetstream_err("in-progress failed", e)))
    }
}

impl js::HostMessageHandle for ActiveCtx<'_> {
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
        Ok(self.table.get(&rep)?.sequence)
    }

    async fn delivery_count(&mut self, rep: Resource<MessageHandle>) -> wasmtime::Result<u32> {
        Ok(self.table.get(&rep)?.delivery_count)
    }

    async fn drop(&mut self, rep: Resource<MessageHandle>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────
// jetstream.pull-consumer
// ──────────────────────────────────────────────────────────────────────────

/// The shared body of both `fetch` variants. `max_bytes` of 0 means no byte bound.
async fn fetch_batch<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
    rep: Resource<PullConsumerHandle>,
    batch: u32,
    max_bytes: u64,
    timeout_ms: u32,
) -> wasmtime::Result<Result<Vec<Resource<MessageHandle>>, types::NatsError>> {
    let consumer = accessor.with(|mut a| consumer_ref(&mut a, &rep))?;
    let Some(consumer) = consumer else {
        return Ok(Err(types::NatsError::Unexpected(
            "pull consumer has been dropped".to_string(),
        )));
    };

    let mut fetch = consumer
        .fetch()
        .max_messages(batch as usize)
        .expires(Duration::from_millis(timeout_ms as u64));
    if max_bytes > 0 {
        fetch = fetch.max_bytes(max_bytes.min(usize::MAX as u64) as usize);
    }
    let mut stream = match fetch.messages().await {
        Ok(s) => s,
        Err(e) => return Ok(Err(jetstream_err("fetch failed", e))),
    };

    let mut fetched = Vec::new();
    while let Some(next) = stream.next().await {
        match next {
            Ok(msg) => {
                let (sequence, delivery_count) = match msg.info() {
                    Ok(info) => (info.stream_sequence, info.delivered as u32),
                    Err(_) => (0, 1),
                };
                // Pull consumers are always guest-driven, so the acker goes to
                // the handle regardless of ack-mode.
                let (message, acker) = msg.split();
                fetched.push(MessageHandle {
                    acker: Some(Arc::new(acker)),
                    message,
                    sequence,
                    delivery_count,
                });
            }
            Err(e) => {
                warn!("pull-consumer fetch stream error: {e}");
                break;
            }
        }
    }

    if fetched.is_empty() {
        return Ok(Err(types::NatsError::NoMessages));
    }
    let handles = accessor.with(|mut a| -> wasmtime::Result<_> {
        let access = a.get();
        fetched
            .into_iter()
            .map(|h| access.table.push(h))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    })?;
    Ok(Ok(handles))
}

impl<T: 'static + Send> js::HostPullConsumerWithStore<T> for SharedCtx {
    async fn fetch(
        accessor: &Accessor<T, Self>,
        rep: Resource<PullConsumerHandle>,
        batch: u32,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<Vec<Resource<MessageHandle>>, types::NatsError>> {
        fetch_batch(accessor, rep, batch, 0, timeout_ms).await
    }

    async fn fetch_with_limits(
        accessor: &Accessor<T, Self>,
        rep: Resource<PullConsumerHandle>,
        batch: u32,
        max_bytes: u64,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<Vec<Resource<MessageHandle>>, types::NatsError>> {
        fetch_batch(accessor, rep, batch, max_bytes, timeout_ms).await
    }

    async fn info(
        accessor: &Accessor<T, Self>,
        rep: Resource<PullConsumerHandle>,
    ) -> wasmtime::Result<Result<js::ConsumerInfo, types::NatsError>> {
        let consumer = accessor.with(|mut a| consumer_ref(&mut a, &rep))?;
        let Some(mut consumer) = consumer else {
            return Ok(Err(types::NatsError::Unexpected(
                "pull consumer has been dropped".to_string(),
            )));
        };
        match consumer.info().await {
            Ok(info) => Ok(Ok(consumer_info_to_wit(info))),
            Err(e) => Ok(Err(jetstream_err("consumer info failed", e))),
        }
    }
}

impl js::HostPullConsumer for ActiveCtx<'_> {
    async fn drop(&mut self, rep: Resource<PullConsumerHandle>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────
// kv
// ──────────────────────────────────────────────────────────────────────────

impl<T: 'static + Send> kv::HostWithStore<T> for SharedCtx {
    async fn open(
        accessor: &Accessor<T, Self>,
        bucket: String,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        if let Err(e) = check_bucket(&conn, &bucket) {
            return Ok(Err(e));
        }

        let store = match conn.jetstream.get_key_value(&bucket).await {
            Ok(store) => store,
            Err(e) => {
                return Ok(Err(types::NatsError::NotFound(format!(
                    "bucket '{bucket}': {e}"
                ))));
            }
        };
        let resource = accessor.with(|mut a| a.get().table.push(BucketHandle { store }))?;
        Ok(Ok(resource))
    }
}

impl kv::Host for ActiveCtx<'_> {}

impl<T: 'static + Send> kv::HostBucketWithStore<T> for SharedCtx {
    async fn get(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<kv::Entry, types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        match store.entry(&key).await {
            Ok(Some(e)) if e.operation == jetstream::kv::Operation::Put => {
                Ok(Ok(kv_entry_to_wit(&e)))
            }
            // A delete or purge tombstone is still an absent key.
            Ok(_) => Ok(Err(types::NatsError::KeyNotFound)),
            Err(e) => Ok(Err(jetstream_err("kv get failed", e))),
        }
    }

    async fn put(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        Ok(store
            .put(&key, value.into())
            .await
            .map_err(|e| jetstream_err("kv put failed", e)))
    }

    async fn create(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        Ok(store
            .create(&key, value.into())
            .await
            .map_err(|e| jetstream_err("kv create failed", e)))
    }

    async fn update(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
        expected_revision: u64,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        match store.update(&key, value.into(), expected_revision).await {
            Ok(rev) => Ok(Ok(rev)),
            Err(e) => {
                if e.to_string()
                    .to_ascii_lowercase()
                    .contains("wrong last sequence")
                {
                    let actual = store
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
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        Ok(store
            .delete(&key)
            .await
            .map_err(|e| jetstream_err("kv delete failed", e)))
    }

    async fn purge(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        Ok(store
            .purge(&key)
            .await
            .map_err(|e| jetstream_err("kv purge failed", e)))
    }

    async fn keys(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
    ) -> wasmtime::Result<Result<Vec<String>, types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        let mut iter = match store.keys().await {
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
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<Vec<kv::Entry>, types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        let mut hist = match store.history(&key).await {
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
        if out.is_empty() {
            return Ok(Err(types::NatsError::KeyNotFound));
        }
        Ok(Ok(out))
    }

    async fn status(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
    ) -> wasmtime::Result<Result<kv::BucketStatus, types::NatsError>> {
        let mut store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        // `Store::status` reports the stream info cached when the bucket was
        // opened, so writes made through this same handle read back as zero.
        let info = match store.stream.info().await {
            Ok(info) => info.clone(),
            Err(e) => return Ok(Err(jetstream_err("kv status failed", e))),
        };
        Ok(Ok(kv::BucketStatus {
            bucket: store.name.clone(),
            values: info.state.messages,
            history: info
                .config
                .max_messages_per_subject
                .clamp(0, u8::MAX as i64) as u8,
            ttl_seconds: info.config.max_age.as_secs(),
            bytes: info.state.bytes,
        }))
    }
}

impl kv::HostBucket for ActiveCtx<'_> {
    async fn drop(&mut self, rep: Resource<BucketHandle>) -> wasmtime::Result<()> {
        self.table.delete(rep)?;
        Ok(())
    }
}
