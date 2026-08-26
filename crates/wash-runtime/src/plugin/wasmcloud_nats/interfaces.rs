//! # `wasmcloud:nats@0.1.0` host implementations
//!
//! Every function in the package is an `async func`, so this is the only
//! implementation: a sync-signature function cannot be lifted with the async
//! canonical ABI, so one package cannot serve both a P2 and a P3 guest, and
//! this interface is a P3 interface.
//!
//! Because the WIT functions are `async func`s the generated host traits use
//! wasmtime's *concurrent* ABI: methods are `async fn`s on `SharedCtx` taking
//! an [`Accessor`], rather than `&mut self` methods on `ActiveCtx`. For a guest
//! that means a `request` no longer blocks the instance, and a handler can
//! await a KV read while the host keeps delivering.
//!
//! Every binding of a workload runs off its own [`ConnHandle`]: one connection,
//! one grant, and one set of limits per `(implements ..)` name.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::consumer::FromConsumer as _;
use bytes::Bytes;
use futures::StreamExt;
use tracing::{debug, warn};
use wasmtime::component::{Accessor, Resource};

use crate::engine::ctx::{ActiveCtx, SharedCtx};

use super::conn::{ConnHandle, server_at_least};
use super::handles::{BucketHandle, MessageHandle, PullConsumerHandle};
use super::policy::Denied;
use super::{PLUGIN_NATS_ID, WasmcloudNats};

pub(super) mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "nats-imports",
        imports: { default: async | trappable | tracing },
        named_imports: {
            "wasmcloud:nats/core@0.1.0": super::NatsId,
            "wasmcloud:nats/jetstream@0.1.0": super::NatsId,
            "wasmcloud:nats/kv@0.1.0": super::NatsId,
        },
        with: {
            "wasmcloud:nats/jetstream@0.1.0.message-handle": super::super::handles::MessageHandle,
            "wasmcloud:nats/jetstream@0.1.0.pull-consumer": super::super::handles::PullConsumerHandle,
            "wasmcloud:nats/kv@0.1.0.bucket": super::super::handles::BucketHandle,
        },
    });
}

use bindings::wasmcloud::nats::{core, jetstream as js, kv, types};
// The label-routed twins of the three routable interfaces. Every method takes
// the NatsId its `(implements ..)` label resolved to, so a component can import
// `wasmcloud:nats` twice — one label per cluster — and have each call leave on
// that binding's connection, under that binding's grant.
use bindings::named_imports::wasmcloud::nats::{
    core as labeled_core, jetstream as labeled_js, kv as labeled_kv,
};

/// The routing id threaded through every host method of a labeled
/// (`(implements ..)`) nats import: the connection that import is bound to.
pub type NatsId = Arc<ConnHandle>;

/// Upper bound on messages one `scan` may buffer into host memory.
const MAX_SCAN_MESSAGES: usize = 1_000;
/// Wall-clock bound on one `scan`, so a slow stream cannot pin the call open.
const MAX_SCAN_DURATION: Duration = Duration::from_secs(10);
/// Upper bound on subjects one `list-stream-subjects` may buffer.
///
/// Subject *cardinality* sizes the allocation here, not message count: a stream
/// on a per-order or per-device subject scheme holds millions of distinct
/// subjects, and draining the whole map would cost hundreds of megabytes of
/// host Strings before a list lift the guest would likely trap on.
const MAX_STREAM_SUBJECTS: usize = 1_000;
/// Wall-clock bound on one `history` drain, for the same reason `scan` has one.
const MAX_HISTORY_DURATION: Duration = Duration::from_secs(10);
/// Cap on keys returned by one `keys` call.
const KV_KEYS_BATCH: usize = 1000;
/// The server release that taught `$JS.API.STREAM.INFO` to honour a subjects
/// filter. Below it the field is ignored and the response simply carries no
/// subject map, which would otherwise reach the guest as an empty result.
const SUBJECT_FILTER_FLOOR: (u64, u64, u64) = (2, 7, 2);

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

/// Checks a `publish` subject, admitting a reply to an inbox the host itself
/// just handed the guest.
///
/// A responder is granted the subjects it serves, never the random `_INBOX` the
/// requester picked, so without this the core request-reply pattern is only
/// deployable behind an `_INBOX.>` grant — which would also let the workload
/// read every other client's replies. Only a plain not-granted refusal is
/// reconsidered: a reserved prefix or a wildcard stays denied, so a hostile
/// requester cannot aim `reply-to` at `$SYS.>` and use the responder as a
/// confused deputy. The grant is one-shot, so it cannot be replayed.
fn check_publish_subject(conn: &ConnHandle, subject: &str) -> Result<(), types::NatsError> {
    match conn.policy.check_subject(subject) {
        Ok(()) => Ok(()),
        Err(Denied::NotGranted) if conn.take_reply_grant(subject) => Ok(()),
        Err(reason) => Err(denied(reason, types::DeniedResource::Subject, subject)),
    }
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

// The 0.2.0 twins of the shared error classifiers, over this revision's
// generated `types`. One body, two revisions — see the macro's own docs.
super::handles::nats_error_classifiers!();

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
    // Read once: the limit is live, so two reads could straddle a reconnect
    // and report a limit the check did not use.
    let max_payload = conn.max_payload();
    if max_payload > 0 && size as u64 > max_payload {
        return Err(types::NatsError::MaxPayloadExceeded(max_payload));
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

/// True when a failed `update` was refused by the CAS check rather than by
/// anything else.
///
/// The typed kind is authoritative; the string match stays as a fallback for
/// a server (or a client) that reports the rejection without one.
fn is_revision_mismatch(e: &jetstream::kv::UpdateError) -> bool {
    matches!(e.kind(), jetstream::kv::UpdateErrorKind::WrongLastRevision)
        || e.to_string()
            .to_ascii_lowercase()
            .contains("wrong last sequence")
}

/// Reads the sequence out of the server's `wrong last sequence: <N>` rejection.
fn parse_wrong_last_sequence(description: &str) -> Option<u64> {
    let lowered = description.to_ascii_lowercase();
    let tail = lowered.split_once("wrong last sequence")?.1;
    let digits: String = tail
        .trim_start()
        .trim_start_matches(':')
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
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
        // Reported alongside the singular field rather than folded into it: a
        // consumer provisioned with the plural form leaves the singular one
        // empty, and empty is how the WIT says "captures the whole stream".
        // Joining the list into one string would only mislead subject parsers.
        filter_subjects: info.config.filter_subjects.clone(),
        max_ack_pending: info.config.max_ack_pending.max(0) as u64,
        max_waiting: info.config.max_waiting.max(0) as u64,
        // The two a guest has to size a pull against: over either, the server
        // refuses the request outright rather than trimming it.
        max_request_batch: info.config.max_batch.max(0) as u64,
        max_request_max_bytes: info.config.max_bytes.max(0) as u64,
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

impl<T: 'static + Send> labeled_core::HostWithStore<T> for SharedCtx {
    async fn publish(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        msg: types::NatsMessage,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let conn = id;

        let types::NatsMessage {
            subject,
            body,
            reply_to,
            headers,
        } = msg;
        if let Err(e) = check_publish_subject(&conn, &subject) {
            return Ok(Err(e));
        }
        // The reply-to the guest asks responses on is a subject it is inviting
        // traffic to, not one the host handed it, so it gets no carve-out.
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

        Ok(result.map_err(|e| core_publish_err("failed to publish", e, conn.max_payload())))
    }

    async fn request(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        msg: types::NatsMessage,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<types::NatsMessage, types::NatsError>> {
        let conn = id;

        let types::NatsMessage {
            subject,
            reply_to,
            body,
            headers,
        } = msg;
        if let Err(e) = check_subject(&conn, &subject) {
            return Ok(Err(e));
        }
        // The reply subject is the host's to choose: replies land on the
        // per-workload inbox so two workloads on one host cannot observe each
        // other's responses. Forwarding a received message as a request is a
        // legitimate pattern, so this is a diagnostic rather than a warning.
        if reply_to.is_some() {
            debug!(
                %subject,
                "request: caller-supplied reply-to is ignored; replies use the per-workload inbox"
            );
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
                            types::NatsError::MaxPayloadExceeded(conn.max_payload())
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

/// Deletes `scan`'s ephemeral consumer if the host future never reaches its
/// in-line cleanup.
///
/// Under the concurrent ABI a guest can cancel a scan subtask, and a stopping
/// workload drops every host future it has in flight; either way the future is
/// abandoned at an await point and nothing after it runs. Without this the
/// consumer sits on the stream skewing `consumer_count` and pressuring
/// `max_consumers` until the server reaps it — exactly what a burst of
/// cancelled scans produces.
struct ScanConsumerGuard(Option<(jetstream::stream::Stream, String)>);

impl ScanConsumerGuard {
    fn arm(stream: &jetstream::stream::Stream, name: &str) -> Self {
        Self(Some((stream.clone(), name.to_string())))
    }

    /// Hands responsibility back once the in-line delete has completed, so the
    /// consumer is never deleted twice.
    fn defuse(&mut self) {
        self.0 = None;
    }
}

impl Drop for ScanConsumerGuard {
    fn drop(&mut self) {
        let Some((stream, name)) = self.0.take() else {
            return;
        };
        // Drop is synchronous and the delete is a round trip, so it has to
        // outlive this frame.
        tokio::spawn(async move {
            if let Err(e) = stream.delete_consumer(&name).await {
                warn!("failed to clean up scan consumer after cancellation: {e}");
            }
        });
    }
}

impl<T: 'static + Send> labeled_js::HostWithStore<T> for SharedCtx {
    async fn publish(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        msg: types::NatsMessage,
    ) -> wasmtime::Result<Result<js::PublishAck, types::NatsError>> {
        let conn = id;

        if let Err(e) = check_subject(&conn, &msg.subject) {
            return Ok(Err(e));
        }
        // JetStream answers a publish on a reply subject the client owns, so a
        // caller-supplied one has nowhere to go.
        if msg.reply_to.is_some() {
            debug!(
                subject = %msg.subject,
                "jetstream publish: caller-supplied reply-to is ignored"
            );
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
            Err(e) => {
                return Ok(Err(js_publish_err(
                    "failed to publish",
                    e,
                    conn.max_payload(),
                )));
            }
        };

        match ack_future.await {
            Ok(ack) => Ok(Ok(js::PublishAck {
                stream_name: ack.stream,
                sequence: ack.sequence,
                duplicate: ack.duplicate,
            })),
            Err(e) => Ok(Err(js_publish_err(
                "failed to confirm publish",
                e,
                conn.max_payload(),
            ))),
        }
    }

    async fn get_by_sequence(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        stream_name: String,
        sequence: u64,
    ) -> wasmtime::Result<Result<js::StoredMessage, types::NatsError>> {
        let conn = id;
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
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
        };

        match stream.direct_get(sequence).await {
            Ok(m) => {
                // The stream grant got us this far; the subject grant decides
                // whether this particular message is one the workload may
                // read, the same boundary a consumer filter is held to. The
                // refusal names the sequence rather than the subject it found
                // there, so a caller cannot walk sequences to learn the names
                // of the subjects it was not granted.
                if conn.policy.check_stored_subject(&m.subject).is_err() {
                    return Ok(Err(denied(
                        Denied::NotGranted,
                        types::DeniedResource::Message,
                        &format!("{stream_name}#{sequence}"),
                    )));
                }
                Ok(Ok(js::StoredMessage {
                    subject: m.subject.to_string(),
                    sequence: m.sequence,
                    data: m.payload.to_vec(),
                    headers: Some(nats_headers_to_wit(&m.headers)),
                }))
            }
            Err(e) => Ok(Err(jetstream_err("get-by-sequence failed", e))),
        }
    }

    async fn scan(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        stream_name: String,
        start_sequence: u64,
        max_count: u32,
    ) -> wasmtime::Result<Result<Vec<js::StoredMessage>, types::NatsError>> {
        let conn = id;
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
        };

        let effective_start = start_sequence.max(1);
        let pull_consumer = match stream
            .create_consumer(jetstream::consumer::pull::Config {
                deliver_policy: jetstream::consumer::DeliverPolicy::ByStartSequence {
                    start_sequence: effective_start,
                },
                // Belt to the braces of the cleanup below: anything that still
                // escapes both the in-line delete and the drop guard — a host
                // that dies mid-scan — is reaped by the server instead of
                // counting against the stream's `max_consumers` forever. Sits
                // comfortably above MAX_SCAN_DURATION so a slow but healthy
                // scan is never reaped out from under itself.
                inactive_threshold: Duration::from_secs(30),
                ..Default::default()
            })
            .await
        {
            Ok(c) => c,
            Err(e) => return Ok(Err(jetstream_err("failed to create scan consumer", e))),
        };
        // The delete below only runs if this future is still being polled. A
        // guest that cancels the scan subtask, or a workload that stops, drops
        // it at an await point instead, and the guard is what deletes the
        // consumer on that path.
        let mut cleanup = ScanConsumerGuard::arm(&stream, &pull_consumer.cached_info().name);

        // Read inside a block so every exit — including the error ones — falls
        // through to the cleanup below with the message stream already dropped.
        let collected: Result<Vec<js::StoredMessage>, types::NatsError> = async {
            let mut msg_stream = pull_consumer
                .messages()
                .await
                .map_err(|e| jetstream_err("failed to get messages", e))?;

            let mut messages = Vec::new();
            let limit = (max_count as usize).min(MAX_SCAN_MESSAGES);
            let deadline = tokio::time::Instant::now() + MAX_SCAN_DURATION;

            while messages.len() < limit && tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_millis(100), msg_stream.next()).await {
                    Ok(Some(Ok(msg))) => {
                        // A stream grant is authority over the stream, not
                        // over every subject stored in it: the declarative
                        // path holds a consumer's filter to `subject-allow`,
                        // and reading the same messages directly answers the
                        // same way. Skipped messages are not counted against
                        // the limit, so the result is what a server-side
                        // filtered consumer would have delivered — a full
                        // batch whose sequences may gap.
                        if conn.policy.check_stored_subject(&msg.subject).is_err() {
                            continue;
                        }
                        let sequence = msg
                            .info()
                            .map_err(|e| jetstream_err("failed to get message info", e))?
                            .stream_sequence;
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
            Ok(messages)
        }
        .await;

        // The consumer is ephemeral to this call; without this each scan leaves
        // one behind on the stream.
        if let Err(e) = stream
            .delete_consumer(&pull_consumer.cached_info().name)
            .await
        {
            warn!("failed to clean up scan consumer: {e}");
        }
        cleanup.defuse();

        let mut messages = match collected {
            Ok(m) => m,
            Err(e) => return Ok(Err(e)),
        };
        messages.sort_by_key(|m| m.sequence);
        Ok(Ok(messages))
    }

    async fn open_pull_consumer(
        accessor: &Accessor<T, Self>,
        id: NatsId,
        stream_name: String,
        consumer: String,
    ) -> wasmtime::Result<Result<Resource<PullConsumerHandle>, types::NatsError>> {
        let conn = id;
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
        };

        // Attach only: `create_consumer` is create-or-update and would rewrite
        // an existing durable's filter subject and deliver policy.
        //
        // Spelled out rather than left to `get_consumer`, which folds the
        // lookup and the config conversion into one `crate::Error` and erases
        // the difference between "no such consumer", "the API did not answer",
        // and "it exists but it is a push consumer" — three conditions a guest
        // has to tell apart, and only the first of which is `not-found`.
        let info = match stream.consumer_info(&consumer).await {
            Ok(i) => i,
            Err(e) => return Ok(Err(consumer_lookup_err(&stream_name, &consumer, e))),
        };
        let config = match jetstream::consumer::pull::Config::try_from_consumer_config(
            info.config.clone(),
        ) {
            Ok(c) => c,
            Err(e) => {
                return Ok(Err(types::NatsError::Jetstream(format!(
                    "consumer '{consumer}' on stream '{stream_name}' exists but is not a pull consumer: {e}"
                ))));
            }
        };
        let opened = jetstream::consumer::Consumer::new(config, info, conn.jetstream.clone());

        let resource = accessor.with(|mut a| {
            a.get().table.push(PullConsumerHandle {
                consumer: Some(opened),
            })
        })?;
        Ok(Ok(resource))
    }

    async fn get_stream_info(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        stream_name: String,
    ) -> wasmtime::Result<Result<js::StreamInfo, types::NatsError>> {
        let conn = id;
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
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
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        stream_name: String,
        subject_filter: String,
    ) -> wasmtime::Result<Result<Vec<js::SubjectCount>, types::NatsError>> {
        let conn = id;
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }
        // An older server does not reject the subjects filter, it ignores it
        // and answers with no subject map — which would reach the guest as an
        // authoritative empty list. Say the call is unavailable instead.
        let version = conn.server_version();
        if !server_at_least(&version, SUBJECT_FILTER_FLOOR) {
            let (major, minor, patch) = SUBJECT_FILTER_FLOOR;
            return Ok(Err(types::NatsError::UnsupportedByServer(format!(
                "subject filtering of stream info requires NATS server {major}.{minor}.{patch} or \
                 newer; connected server is {version}"
            ))));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
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

        // Bounded on both axes, as every other collecting endpoint here is:
        // the paging walk keeps the call open for as long as the server has
        // pages, and the subject map is the one result whose size the guest's
        // arguments do not cap.
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + MAX_SCAN_DURATION;
        loop {
            match tokio::time::timeout_at(deadline, info.next()).await {
                Ok(Some(Ok((subject, count)))) => {
                    out.push(js::SubjectCount {
                        subject,
                        count: count as u64,
                    });
                    if out.len() >= MAX_STREAM_SUBJECTS {
                        warn!(
                            "list-stream-subjects truncated at {MAX_STREAM_SUBJECTS} entries — stream has more"
                        );
                        break;
                    }
                }
                Ok(Some(Err(e))) => return Ok(Err(jetstream_err("stream subjects failed", e))),
                Ok(None) => break,
                Err(_) => {
                    warn!(
                        "list-stream-subjects timed out after {MAX_SCAN_DURATION:?} — result truncated"
                    );
                    break;
                }
            }
        }
        out.sort_by(|a, b| a.subject.cmp(&b.subject));
        Ok(Ok(out))
    }

    async fn get_consumer_info(
        _accessor: &Accessor<T, Self>,
        id: NatsId,
        stream_name: String,
        consumer: String,
    ) -> wasmtime::Result<Result<js::ConsumerInfo, types::NatsError>> {
        let conn = id;
        if let Err(e) = check_stream(&conn, &stream_name) {
            return Ok(Err(e));
        }

        let stream = match conn.jetstream.get_stream(&stream_name).await {
            Ok(s) => s,
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
        };
        match stream.consumer_info(&consumer).await {
            Ok(info) => Ok(Ok(consumer_info_to_wit(&info))),
            Err(e) => Ok(Err(consumer_lookup_err(&stream_name, &consumer, e))),
        }
    }
}

impl js::Host for ActiveCtx<'_> {}

// ──────────────────────────────────────────────────────────────────────────
// jetstream.message-handle
// ──────────────────────────────────────────────────────────────────────────

/// Clones the handle's acker, and the settled flag alongside it, leaving both
/// on the handle.
///
/// Cloning rather than taking is what makes a failed settle retryable. A settle
/// that never reached the server is exactly the case a guest should retry — and
/// the documented remedy for an `ack-sync` that timed out — but a handle
/// emptied before the wire operation reports "already settled" on the retry,
/// which reads as "the ack landed". The guest stops, the message redelivers,
/// and the side effect runs twice.
fn clone_acker<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
    rep: &Resource<MessageHandle>,
) -> wasmtime::Result<Option<(Arc<jetstream::message::Acker>, Arc<AtomicBool>)>> {
    accessor.with(|mut a| {
        let handle = a.get().table.get(rep)?;
        let settled = handle.settled.clone();
        Ok(handle.acker.clone().map(|acker| (acker, settled)))
    })
}

/// Retires the acker once the server has taken the settle, making the handle
/// one-shot from that point on.
///
/// Taking an acker that a concurrent settle already took is a harmless no-op,
/// and a handle that vanished from the table is nothing left to clear.
fn clear_acker<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
    rep: &Resource<MessageHandle>,
) {
    accessor.with(|mut a| {
        if let Ok(handle) = a.get().table.get_mut(rep) {
            handle.acker.take();
        }
    });
}

/// Settles a message with `kind`, retiring the handle's acker once the server
/// has taken it.
///
/// `retires` says whether this settle means the message never has to come round
/// again — true for a term, false for a nak, which asks for redelivery.
async fn settle<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
    rep: Resource<MessageHandle>,
    kind: jetstream::AckKind,
    retires: bool,
    what: &'static str,
) -> wasmtime::Result<Result<(), types::NatsError>> {
    let Some((acker, settled)) = clone_acker(accessor, &rep)? else {
        return Ok(Err(already_settled()));
    };
    match acker.ack_with(kind).await {
        Ok(()) => {
            clear_acker(accessor, &rep);
            if retires {
                settled.store(true, Ordering::Release);
            }
            Ok(Ok(()))
        }
        Err(e) => Ok(Err(jetstream_err(format!("{what} failed"), e))),
    }
}

impl<T: 'static + Send> js::HostMessageHandleWithStore<T> for SharedCtx {
    async fn ack(
        accessor: &Accessor<T, Self>,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let Some((acker, settled)) = clone_acker(accessor, &rep)? else {
            return Ok(Err(already_settled()));
        };
        match acker.ack().await {
            Ok(()) => {
                clear_acker(accessor, &rep);
                settled.store(true, Ordering::Release);
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(jetstream_err("ack failed", e))),
        }
    }

    async fn ack_sync(
        accessor: &Accessor<T, Self>,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let Some((acker, settled)) = clone_acker(accessor, &rep)? else {
            return Ok(Err(already_settled()));
        };
        match acker.double_ack().await {
            Ok(()) => {
                clear_acker(accessor, &rep);
                settled.store(true, Ordering::Release);
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(jetstream_err("ack-sync failed", e))),
        }
    }

    async fn nak(
        accessor: &Accessor<T, Self>,
        rep: Resource<MessageHandle>,
        delay_ms: Option<u32>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let kind = jetstream::AckKind::Nak(delay_ms.map(|ms| Duration::from_millis(ms as u64)));
        settle(accessor, rep, kind, false, "nak").await
    }

    async fn term(
        accessor: &Accessor<T, Self>,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        // A term is the guest saying the message must not come round again, so
        // it retires the sequence just as an ack does.
        settle(accessor, rep, jetstream::AckKind::Term, true, "term").await
    }

    /// Extends ack-wait without settling anything, so it reads the handle's
    /// `progress` acker rather than its `acker`.
    ///
    /// The two differ under `ack-mode: auto`, where the host owns settlement
    /// and `acker` is `None`: gating this on settlement ownership left the
    /// default mode with no way to extend ack-wait at all, so a handler slower
    /// than the 30s ack-wait was redelivered and run twice while the WIT-
    /// sanctioned mitigation returned "already settled". `progress` is
    /// populated in both modes; `None` now means only a delivery that never
    /// carried an acker, such as a `scan` result.
    async fn in_progress(
        accessor: &Accessor<T, Self>,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let acker = accessor.with(|mut a| -> wasmtime::Result<_> {
            Ok(a.get().table.get(&rep)?.progress.clone())
        })?;
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

/// How a pull request ended, read off the status message the server closes the
/// batch with.
///
/// async-nats surfaces those statuses as a terminal stream error carrying the
/// code and the server's description, and nothing else reports them: without
/// this a refused request is indistinguishable from an idle consumer, and a
/// byte-capped batch from a drained one.
pub(super) fn classify_pull_end(error: &str) -> PullEnd {
    // A 409 is either a refusal of the whole request (nothing was delivered)
    // or the byte bound closing an admitted batch.
    if !error.contains("409") {
        return PullEnd::Failed;
    }
    if error.contains("Exceeds MaxBytes") || error.contains("Exceeded MaxBytes") {
        return PullEnd::ByteLimit;
    }
    if error.contains("Exceeded Max") {
        return PullEnd::Refused;
    }
    // The consumer is not there to pull from any more. Reporting this as an
    // idle consumer would tell a guest to keep waiting on something that can
    // never answer.
    if error.contains("Consumer Deleted") || error.contains("Consumer is push based") {
        return PullEnd::Gone;
    }
    // Every other 409 is the server standing the request down — a shutdown, a
    // leadership change. Transient, but not the same as having nothing to give.
    PullEnd::Interrupted
}

/// The outcome [`classify_pull_end`] reads out of a terminal batch error.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PullEnd {
    /// The server refused the request: it asks for more than the consumer's
    /// `max-request-batch` / `max-request-max-bytes` / `max-waiting` allows.
    Refused,
    /// A byte bound closed the batch after delivering what fit.
    ByteLimit,
    /// The consumer no longer serves pulls: deleted out of band, or replaced by
    /// a push consumer of the same name.
    Gone,
    /// The server stood the request down mid-flight — a shutdown or a
    /// leadership change. Retryable, unlike a refusal.
    Interrupted,
    /// Anything else — a transport error, or a status this host does not model.
    Failed,
}

/// The shared body of both `fetch` variants. `max_bytes` of 0 means no byte bound.
async fn fetch_batch<T: 'static + Send>(
    accessor: &Accessor<T, SharedCtx>,
    rep: Resource<PullConsumerHandle>,
    batch: u32,
    max_bytes: u64,
    timeout_ms: u32,
) -> wasmtime::Result<Result<js::FetchedBatch, types::NatsError>> {
    // Refused before the builder exists, so no batch:0 pull request reaches the
    // server. async-nats would terminate such a batch immediately and the empty
    // result would surface as `no-messages` — "the timeout elapsed empty" — so
    // a guest whose computed batch reached zero would read a consumer holding
    // hundreds of pending messages as drained.
    if batch == 0 {
        return Ok(Err(types::NatsError::Unexpected(
            "fetch batch must be >= 1 (got 0)".to_string(),
        )));
    }

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
    let mut ended = None;
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
                let acker = Arc::new(acker);
                fetched.push(MessageHandle {
                    acker: Some(acker.clone()),
                    progress: Some(acker),
                    settled: Arc::new(AtomicBool::new(false)),
                    message,
                    sequence,
                    delivery_count,
                });
            }
            Err(e) => {
                let detail = e.to_string();
                ended = Some(classify_pull_end(&detail));
                match ended {
                    // A refusal is the guest's to fix — the request asks for
                    // more than the consumer allows — so it must not read as
                    // an idle consumer.
                    Some(PullEnd::Refused) => {
                        return Ok(Err(types::NatsError::LimitExceeded(detail)));
                    }
                    Some(PullEnd::ByteLimit) => debug!("pull batch closed by a byte bound: {e}"),
                    // What was already delivered is real and worth returning;
                    // only an empty batch has to carry the reason as an error,
                    // since there is nothing else to carry it.
                    Some(PullEnd::Gone) if fetched.is_empty() => {
                        return Ok(Err(types::NatsError::NotFound(detail)));
                    }
                    Some(PullEnd::Gone) => warn!("pull consumer went away mid-batch: {e}"),
                    Some(PullEnd::Interrupted) if fetched.is_empty() => {
                        return Ok(Err(jetstream_err("pull request was stood down", &detail)));
                    }
                    Some(PullEnd::Interrupted) => {
                        warn!("server stood the pull request down: {e}")
                    }
                    _ => warn!("pull-consumer fetch stream error: {e}"),
                }
                break;
            }
        }
    }

    if fetched.is_empty() {
        return match ended {
            // The byte bound admitted nothing: the message at the head is
            // bigger than the bound itself, and retrying unchanged loops
            // forever. Say so rather than reporting an idle consumer.
            Some(PullEnd::ByteLimit) => Ok(Err(types::NatsError::LimitExceeded(
                "the next message is larger than the requested max-bytes".to_string(),
            ))),
            _ => Ok(Err(types::NatsError::NoMessages)),
        };
    }
    // Why the batch ended, so a guest can tell "that is everything" from "there
    // is more, the byte bound stopped us". A batch cut short by a transport
    // error keeps the messages — they are already delivered, and dropping them
    // would only wait out ack-wait for a redelivery — and reports `drained`,
    // the one case where the reason is logged rather than typed.
    let stop = match ended {
        Some(PullEnd::ByteLimit) => js::FetchStop::ByteLimit,
        _ if fetched.len() >= batch as usize => js::FetchStop::BatchFilled,
        _ => js::FetchStop::Drained,
    };
    // Table exhaustion is recoverable, not a reason to kill the instance: every
    // other failure on this path is a typed error, and the push subscriber
    // already treats the identical failure as warn-nak-continue. Keep the
    // ackers first — a handle consumed by a failed `push` is gone, and the
    // whole batch has to be nakked either way.
    let ackers: Vec<Arc<jetstream::message::Acker>> =
        fetched.iter().filter_map(|h| h.acker.clone()).collect();
    let pushed = accessor.with(|mut a| {
        let access = a.get();
        let mut ids = Vec::with_capacity(fetched.len());
        for handle in fetched {
            match access.table.push(handle) {
                Ok(id) => ids.push(id),
                Err(_) => {
                    // A half-pushed batch is worse than none: the guest is told
                    // nothing came back, so nothing would ever drop these.
                    for id in ids {
                        let _ = access.table.delete(id);
                    }
                    return None;
                }
            }
        }
        Some(ids)
    });
    let Some(messages) = pushed else {
        // Nothing reached the guest, so nothing there can settle these. Nak
        // them for immediate redelivery rather than letting them hold
        // max-ack-pending open until ack-wait expires.
        for acker in ackers {
            if let Err(e) = acker.ack_with(jetstream::AckKind::Nak(None)).await {
                warn!("failed to nak a fetched message the guest never received: {e}");
            }
        }
        return Ok(Err(types::NatsError::Unexpected(
            "host resource table full".to_string(),
        )));
    };
    Ok(Ok(js::FetchedBatch { messages, stop }))
}

impl<T: 'static + Send> js::HostPullConsumerWithStore<T> for SharedCtx {
    async fn fetch(
        accessor: &Accessor<T, Self>,
        rep: Resource<PullConsumerHandle>,
        batch: u32,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<js::FetchedBatch, types::NatsError>> {
        fetch_batch(accessor, rep, batch, 0, timeout_ms).await
    }

    async fn fetch_with_limits(
        accessor: &Accessor<T, Self>,
        rep: Resource<PullConsumerHandle>,
        batch: u32,
        max_bytes: u64,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<js::FetchedBatch, types::NatsError>> {
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
        // Classified the same way `get-consumer-info` classifies it: a consumer
        // deleted out from under a live handle has to look the same through
        // both introspection paths, or a guest cannot write one handler for it.
        let (stream_name, consumer_name) = {
            let cached = consumer.cached_info();
            (cached.stream_name.clone(), cached.name.clone())
        };
        match consumer.info().await {
            Ok(info) => Ok(Ok(consumer_info_to_wit(info))),
            Err(e) => Ok(Err(consumer_lookup_err(&stream_name, &consumer_name, e))),
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

impl<T: 'static + Send> labeled_kv::HostWithStore<T> for SharedCtx {
    async fn open(
        accessor: &Accessor<T, Self>,
        id: NatsId,
        bucket: String,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, types::NatsError>> {
        let conn = id;
        if let Err(e) = check_bucket(&conn, &bucket) {
            return Ok(Err(e));
        }

        let store = match conn.jetstream.get_key_value(&bucket).await {
            Ok(store) => store,
            Err(e) => return Ok(Err(bucket_lookup_err(&bucket, e))),
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
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::kv::EntryErrorKind::TimedOut);
                Ok(Err(kv_err("kv get failed", timed_out, e)))
            }
        }
    }

    async fn put(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        let conn = conn_or_return!(accessor);
        // Same oversize condition as a publish, so it has to reach the guest as
        // the same typed error: a guest that switches to chunked storage on
        // `max-payload-exceeded` would otherwise see a generic jetstream fault
        // and retry the doomed write forever.
        if let Err(e) = check_payload(value.len(), None, &conn) {
            return Ok(Err(e));
        }
        Ok(store
            .put(&key, value.into())
            .await
            .map_err(|e| kv_err("kv put failed", chain_timed_out(&e), e)))
    }

    async fn create(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        let conn = conn_or_return!(accessor);
        if let Err(e) = check_payload(value.len(), None, &conn) {
            return Ok(Err(e));
        }
        Ok(store
            .create(&key, value.into())
            .await
            .map_err(|e| kv_err("kv create failed", chain_timed_out(&e), e)))
    }

    async fn update(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
        expected_revision: u64,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        let conn = conn_or_return!(accessor);
        if let Err(e) = check_payload(value.len(), None, &conn) {
            return Ok(Err(e));
        }
        match store.update(&key, value.into(), expected_revision).await {
            Ok(rev) => Ok(Ok(rev)),
            Err(e) if is_revision_mismatch(&e) => {
                // The rejection already names the sequence the server holds
                // ("wrong last sequence: N"), so read it out of the rejection
                // rather than paying a second round trip that a degraded
                // connection would fail anyway.
                if let Some(actual) = parse_wrong_last_sequence(&e.to_string()) {
                    return Ok(Err(types::NatsError::RevisionMismatch(actual)));
                }
                match store.entry(&key).await {
                    Ok(Some(entry)) => Ok(Err(types::NatsError::RevisionMismatch(entry.revision))),
                    // Genuinely empty subject: zero is the real revision here.
                    Ok(None) => Ok(Err(types::NatsError::RevisionMismatch(0))),
                    // Never fabricate a revision. A guest told `revision-mismatch(0)`
                    // retries with `expected-revision: 0` as the WIT instructs, which
                    // against a subject whose real sequence is nonzero re-fails every
                    // time — or blind-creates over an emptied one.
                    Err(_) => Ok(Err(kv_err(
                        "kv update failed",
                        matches!(e.kind(), jetstream::kv::UpdateErrorKind::TimedOut),
                        e,
                    ))),
                }
            }
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::kv::UpdateErrorKind::TimedOut);
                Ok(Err(kv_err("kv update failed", timed_out, e)))
            }
        }
    }

    async fn delete(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        Ok(store.delete(&key).await.map_err(|e| {
            let timed_out = matches!(e.kind(), jetstream::kv::DeleteErrorKind::TimedOut);
            kv_err("kv delete failed", timed_out, e)
        }))
    }

    async fn purge(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        Ok(store.purge(&key).await.map_err(|e| {
            let timed_out = matches!(e.kind(), jetstream::kv::PurgeErrorKind::TimedOut);
            kv_err("kv purge failed", timed_out, e)
        }))
    }

    async fn keys(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
    ) -> wasmtime::Result<Result<kv::KeyPage, types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        let mut iter = match store.keys().await {
            Ok(i) => i,
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::kv::WatchErrorKind::TimedOut);
                return Ok(Err(kv_err("kv keys failed", timed_out, e)));
            }
        };
        let mut out = Vec::new();
        let mut truncated = false;
        while let Some(next) = iter.next().await {
            match next {
                Ok(k) => {
                    out.push(k);
                    // The cap stays — draining an arbitrarily large bucket
                    // into one guest allocation is its own failure mode — but
                    // the walk goes one key past it. That key is the only
                    // evidence the bucket holds more, and dropping it is what
                    // made a partial listing indistinguishable from a whole
                    // one.
                    if out.len() > KV_KEYS_BATCH {
                        warn!("kv keys truncated at {KV_KEYS_BATCH} entries — bucket has more");
                        out.truncate(KV_KEYS_BATCH);
                        truncated = true;
                        break;
                    }
                }
                Err(e) => {
                    let timed_out = chain_timed_out(&e);
                    return Ok(Err(kv_err("kv keys iter failed", timed_out, e)));
                }
            }
        }
        Ok(Ok(kv::KeyPage {
            keys: out,
            truncated,
        }))
    }

    async fn history(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<Vec<kv::Entry>, types::NatsError>> {
        let store = accessor.with(|mut a| store_ref(&mut a, &rep))?;
        // Probe before opening the stream. `Store::history` builds an ordered
        // push consumer that only terminates once it sees an entry reporting
        // zero pending, so a key that holds no messages at all yields nothing
        // and the stream never ends — the call hangs for the connection's
        // lifetime, and a guest retry loop strands one task per attempt.
        // `entry` returns `Ok(None)` for exactly that case: a delete or purge
        // tombstone still comes back as `Some`, and its history still drains.
        match store.entry(&key).await {
            Ok(Some(_)) => {}
            Ok(None) => return Ok(Err(types::NatsError::KeyNotFound)),
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::kv::EntryErrorKind::TimedOut);
                return Ok(Err(kv_err("kv history failed", timed_out, e)));
            }
        }

        let mut hist = match store.history(&key).await {
            Ok(h) => h,
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::kv::WatchErrorKind::TimedOut);
                return Ok(Err(kv_err("kv history failed", timed_out, e)));
            }
        };
        // The probe closes the common case, but history can expire between it
        // and the consumer, so the drain carries its own bound.
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + MAX_HISTORY_DURATION;
        loop {
            match tokio::time::timeout_at(deadline, hist.next()).await {
                Ok(Some(Ok(e))) => out.push(kv_entry_to_wit(&e)),
                Ok(Some(Err(e))) => {
                    let timed_out = chain_timed_out(&e);
                    return Ok(Err(kv_err("kv history iter failed", timed_out, e)));
                }
                Ok(None) => break,
                Err(_) => {
                    return Ok(Err(types::NatsError::Timeout(format!(
                        "kv history did not complete within {MAX_HISTORY_DURATION:?}"
                    ))));
                }
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
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::context::RequestErrorKind::TimedOut);
                return Ok(Err(kv_err("kv status failed", timed_out, e)));
            }
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

// ──────────────────────────────────────────────────────────────────────────
// The plain (unlabeled) route
// ──────────────────────────────────────────────────────────────────────────
//
// A component that imports `wasmcloud:nats` without an `(implements ..)` label
// names no binding, so its calls go out on the workload's unnamed binding —
// the only shape that existed before named bindings, and still the common one.
// Each of these resolves that connection and hands it to the label-routed
// implementation above, so the two routes cannot drift.

impl<T: 'static + Send> core::HostWithStore<T> for SharedCtx {
    async fn publish(
        accessor: &Accessor<T, Self>,
        msg: types::NatsMessage,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_core::HostWithStore<T>>::publish(accessor, conn, msg).await
    }
    async fn request(
        accessor: &Accessor<T, Self>,
        msg: types::NatsMessage,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<types::NatsMessage, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_core::HostWithStore<T>>::request(accessor, conn, msg, timeout_ms).await
    }
}

impl<T: 'static + Send> js::HostWithStore<T> for SharedCtx {
    async fn publish(
        accessor: &Accessor<T, Self>,
        msg: types::NatsMessage,
    ) -> wasmtime::Result<Result<js::PublishAck, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::publish(accessor, conn, msg).await
    }
    async fn get_by_sequence(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        sequence: u64,
    ) -> wasmtime::Result<Result<js::StoredMessage, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::get_by_sequence(
            accessor,
            conn,
            stream_name,
            sequence,
        )
        .await
    }
    async fn scan(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        start_sequence: u64,
        max_count: u32,
    ) -> wasmtime::Result<Result<Vec<js::StoredMessage>, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::scan(
            accessor,
            conn,
            stream_name,
            start_sequence,
            max_count,
        )
        .await
    }
    async fn open_pull_consumer(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        consumer: String,
    ) -> wasmtime::Result<Result<Resource<PullConsumerHandle>, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::open_pull_consumer(
            accessor,
            conn,
            stream_name,
            consumer,
        )
        .await
    }
    async fn get_stream_info(
        accessor: &Accessor<T, Self>,
        stream_name: String,
    ) -> wasmtime::Result<Result<js::StreamInfo, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::get_stream_info(accessor, conn, stream_name).await
    }
    async fn list_stream_subjects(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        subject_filter: String,
    ) -> wasmtime::Result<Result<Vec<js::SubjectCount>, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::list_stream_subjects(
            accessor,
            conn,
            stream_name,
            subject_filter,
        )
        .await
    }
    async fn get_consumer_info(
        accessor: &Accessor<T, Self>,
        stream_name: String,
        consumer: String,
    ) -> wasmtime::Result<Result<js::ConsumerInfo, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_js::HostWithStore<T>>::get_consumer_info(
            accessor,
            conn,
            stream_name,
            consumer,
        )
        .await
    }
}

impl<T: 'static + Send> kv::HostWithStore<T> for SharedCtx {
    async fn open(
        accessor: &Accessor<T, Self>,
        bucket: String,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, types::NatsError>> {
        let conn = conn_or_return!(accessor);
        <Self as labeled_kv::HostWithStore<T>>::open(accessor, conn, bucket).await
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Label-routed resource methods
// ──────────────────────────────────────────────────────────────────────────
//
// A message handle, pull consumer, or bucket already carries the connection it
// was opened through — an `Acker`, a `Consumer`, a `Store` — so its methods
// need no routing and ignore the label. They exist only because the resources
// live in routed interfaces, and delegate to the plain implementations.

impl<T: 'static + Send> labeled_js::HostMessageHandleWithStore<T> for SharedCtx {
    async fn ack(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as js::HostMessageHandleWithStore<T>>::ack(accessor, rep).await
    }
    async fn ack_sync(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as js::HostMessageHandleWithStore<T>>::ack_sync(accessor, rep).await
    }
    async fn nak(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<MessageHandle>,
        delay_ms: Option<u32>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as js::HostMessageHandleWithStore<T>>::nak(accessor, rep, delay_ms).await
    }
    async fn term(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as js::HostMessageHandleWithStore<T>>::term(accessor, rep).await
    }
    async fn in_progress(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as js::HostMessageHandleWithStore<T>>::in_progress(accessor, rep).await
    }
}

impl labeled_js::HostMessageHandle for ActiveCtx<'_> {
    async fn message(
        &mut self,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<types::NatsMessage> {
        <Self as js::HostMessageHandle>::message(self, rep).await
    }
    async fn sequence(
        &mut self,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<u64> {
        <Self as js::HostMessageHandle>::sequence(self, rep).await
    }
    async fn delivery_count(
        &mut self,
        _id: NatsId,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<u32> {
        <Self as js::HostMessageHandle>::delivery_count(self, rep).await
    }
    async fn drop(&mut self, _id: NatsId, rep: Resource<MessageHandle>) -> wasmtime::Result<()> {
        <Self as js::HostMessageHandle>::drop(self, rep).await
    }
}

impl<T: 'static + Send> labeled_js::HostPullConsumerWithStore<T> for SharedCtx {
    async fn fetch(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<PullConsumerHandle>,
        batch: u32,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<js::FetchedBatch, types::NatsError>> {
        <Self as js::HostPullConsumerWithStore<T>>::fetch(accessor, rep, batch, timeout_ms).await
    }
    async fn fetch_with_limits(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<PullConsumerHandle>,
        batch: u32,
        max_bytes: u64,
        timeout_ms: u32,
    ) -> wasmtime::Result<Result<js::FetchedBatch, types::NatsError>> {
        <Self as js::HostPullConsumerWithStore<T>>::fetch_with_limits(
            accessor, rep, batch, max_bytes, timeout_ms,
        )
        .await
    }
    async fn info(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<PullConsumerHandle>,
    ) -> wasmtime::Result<Result<js::ConsumerInfo, types::NatsError>> {
        <Self as js::HostPullConsumerWithStore<T>>::info(accessor, rep).await
    }
}

impl labeled_js::HostPullConsumer for ActiveCtx<'_> {
    async fn drop(
        &mut self,
        _id: NatsId,
        rep: Resource<PullConsumerHandle>,
    ) -> wasmtime::Result<()> {
        <Self as js::HostPullConsumer>::drop(self, rep).await
    }
}

impl<T: 'static + Send> labeled_kv::HostBucketWithStore<T> for SharedCtx {
    async fn get(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<kv::Entry, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::get(accessor, rep, key).await
    }
    async fn put(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::put(accessor, rep, key, value).await
    }
    async fn create(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::create(accessor, rep, key, value).await
    }
    async fn update(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
        expected_revision: u64,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::update(accessor, rep, key, value, expected_revision)
            .await
    }
    async fn delete(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::delete(accessor, rep, key).await
    }
    async fn purge(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::purge(accessor, rep, key).await
    }
    async fn keys(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
    ) -> wasmtime::Result<Result<kv::KeyPage, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::keys(accessor, rep).await
    }
    async fn history(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<Vec<kv::Entry>, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::history(accessor, rep, key).await
    }
    async fn status(
        accessor: &Accessor<T, Self>,
        _id: NatsId,
        rep: Resource<BucketHandle>,
    ) -> wasmtime::Result<Result<kv::BucketStatus, types::NatsError>> {
        <Self as kv::HostBucketWithStore<T>>::status(accessor, rep).await
    }
}

impl labeled_kv::HostBucket for ActiveCtx<'_> {
    async fn drop(&mut self, _id: NatsId, rep: Resource<BucketHandle>) -> wasmtime::Result<()> {
        <Self as kv::HostBucket>::drop(self, rep).await
    }
}

// Marker traits: the interfaces carry no free-standing host state.
impl labeled_core::Host for ActiveCtx<'_> {}
impl labeled_js::Host for ActiveCtx<'_> {}
impl labeled_kv::Host for ActiveCtx<'_> {}

#[cfg(test)]
mod tests {
    use super::{PullEnd, classify_pull_end};

    /// The wording is the server's, relayed by async-nats as a terminal batch
    /// error: these are the exact strings a live nats-server 2.14 produced.
    #[test]
    fn a_refused_request_is_told_from_a_capped_batch() {
        assert_eq!(
            classify_pull_end(
                "error while processing messages from the stream: 409, \
                 Some(\"Exceeded MaxRequestBatch of 5\")"
            ),
            PullEnd::Refused
        );
        assert_eq!(
            classify_pull_end(
                "error while processing messages from the stream: 409, \
                 Some(\"Exceeded MaxRequestMaxBytes of 1024\")"
            ),
            PullEnd::Refused
        );
        assert_eq!(
            classify_pull_end(
                "error while processing messages from the stream: 409, \
                 Some(\"Exceeded MaxWaiting\")"
            ),
            PullEnd::Refused
        );
        // Not a refusal: the batch was admitted and the byte bound ended it.
        assert_eq!(
            classify_pull_end(
                "error while processing messages from the stream: 409, \
                 Some(\"Message Size Exceeds MaxBytes\")"
            ),
            PullEnd::ByteLimit
        );
    }

    /// A consumer that is gone answers with a 409 too, and calling that an idle
    /// consumer sends the guest into a wait that can never end.
    #[test]
    fn a_vanished_consumer_is_told_from_an_idle_one() {
        assert_eq!(
            classify_pull_end("unexpected status code 409: Consumer Deleted"),
            PullEnd::Gone
        );
        assert_eq!(
            classify_pull_end("unexpected status code 409: Consumer is push based"),
            PullEnd::Gone
        );
    }

    /// The remaining 409s are the server standing the request down. Retryable,
    /// but still not "there was nothing there".
    #[test]
    fn a_stood_down_request_is_not_an_empty_one() {
        assert_eq!(
            classify_pull_end("unexpected status code 409: Server Shutdown"),
            PullEnd::Interrupted
        );
        assert_eq!(
            classify_pull_end("unexpected status code 409: Leadership Change"),
            PullEnd::Interrupted
        );
    }

    #[test]
    fn anything_else_stays_a_plain_failure() {
        assert_eq!(
            classify_pull_end("connection reset by peer"),
            PullEnd::Failed
        );
        assert_eq!(
            classify_pull_end(
                "error while processing messages from the stream: 503, Some(\"No Responders\")"
            ),
            PullEnd::Failed
        );
    }
}
