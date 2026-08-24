use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::consumer::FromConsumer as _;
use bytes::Bytes;
use futures::StreamExt;
use tracing::{debug, instrument, warn};
use wasmtime::component::Resource;

use crate::engine::ctx::ActiveCtx;

use super::bindings::wasmcloud::nats0_1_0::{core, jetstream as js, kv, types};
use super::conn::ConnHandle;
use super::handles::{
    BucketHandle, MessageHandle, PullConsumerHandle, already_settled, bucket_lookup_err,
    chain_timed_out, consumer_lookup_err, core_publish_err, jetstream_err, js_publish_err, kv_err,
    stream_lookup_err,
};
use super::policy::Denied;
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
/// Wall-clock bound on one `history` drain, for the same reason: the ordered
/// consumer behind it only ends the stream once the server says nothing is
/// pending, and a subject that empties underneath the call never gets there.
const MAX_HISTORY_DURATION: Duration = Duration::from_secs(10);

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
    // Read once: the limit is live, so two reads could straddle a reconnect
    // and report a limit the check did not use.
    let max_payload = conn.max_payload();
    if max_payload > 0 && size as u64 > max_payload {
        return Err(types::NatsError::MaxPayloadExceeded(max_payload));
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
        if let Err(denial) = conn.policy.check_subject(&subject) {
            // Answering a request means publishing to a random `_INBOX` no
            // grant can name in advance. The host handed that inbox to the
            // guest itself, so it authorizes one reply to it here rather than
            // making responders ask for `_INBOX.>` — a grant broad enough to
            // read every other reply on the connection. Only the
            // not-granted denial is waivable: a requester that could aim
            // `reply-to` at `$JS.>` would otherwise have the guest for a
            // deputy.
            let replying = matches!(denial, Denied::NotGranted) && conn.take_reply_grant(&subject);
            if !replying {
                return Ok(Err(denied(&subject)));
            }
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
            // The client refuses an oversized publish on its own, against a
            // limit that may have shrunk since the pre-check read it. Reporting
            // that as a transport fault would have the guest retry a body that
            // can never fit.
            Err(e) => Ok(Err(core_publish_err(
                "failed to publish",
                e,
                conn.max_payload(),
            ))),
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
            reply_to,
            body,
            headers,
        } = msg;
        if conn.policy.check_subject(&subject).is_err() {
            return Ok(Err(denied(&subject)));
        }
        // The reply subject is the host's to choose: replies land on the
        // per-workload inbox so two workloads on one host cannot observe each
        // other's responses. Forwarding a received message as a request is a
        // legitimate pattern, so this is a diagnostic rather than a warning.
        if reply_to.is_some() {
            debug!(
                "request: caller-supplied reply-to is ignored; replies use the per-workload inbox"
            );
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
                        types::NatsError::MaxPayloadExceeded(conn.max_payload())
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
        // JetStream answers a publish on a reply subject the client owns, so a
        // caller-supplied one has nowhere to go.
        if msg.reply_to.is_some() {
            debug!("jetstream publish: caller-supplied reply-to is ignored");
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

        // Both stages share one error type, and three of its kinds — an ack
        // that never arrived, a subject no stream captures, an oversize body —
        // have WIT variants of their own. Flattened into `jetstream(string)`
        // they read alike, so a guest cannot tell the retry-safe ack timeout
        // from a stream it will never be able to publish to.
        let ack_future = if let Some(header_map) = header_map {
            match conn
                .jetstream
                .publish_with_headers(msg.subject, header_map, msg.body.into())
                .await
            {
                Ok(f) => f,
                Err(e) => {
                    return Ok(Err(js_publish_err(
                        "failed to publish",
                        e,
                        conn.max_payload(),
                    )));
                }
            }
        } else {
            match conn.jetstream.publish(msg.subject, msg.body.into()).await {
                Ok(f) => f,
                Err(e) => {
                    return Ok(Err(js_publish_err(
                        "failed to publish",
                        e,
                        conn.max_payload(),
                    )));
                }
            }
        };

        let ack = match ack_future.await {
            Ok(a) => a,
            Err(e) => {
                return Ok(Err(js_publish_err(
                    "failed to confirm publish",
                    e,
                    conn.max_payload(),
                )));
            }
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

        // Ahead of the lookup: a server that cannot serve direct get at all
        // should say so rather than answer `not-found` for a stream it would
        // never have read anyway, and the round-trip also leaks which streams
        // exist. This is the order the `@0.2.0` twin already uses.
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
                    return Ok(Err(denied(&format!("{stream_name}#{sequence}"))));
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
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
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
                // Comfortably past the scan bound, so anything that still
                // escapes the explicit cleanup below is server-reaped on a
                // known schedule instead of lingering against max-consumers.
                inactive_threshold: Duration::from_secs(30),
                ..Default::default()
            })
            .await
        {
            Ok(c) => c,
            Err(e) => return Ok(Err(jetstream_err("failed to create scan consumer", e))),
        };

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

            loop {
                if messages.len() >= limit || tokio::time::Instant::now() >= deadline {
                    break;
                }
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
                        // A malformed reply is the server's problem, not the
                        // guest's — report it rather than trapping the instance.
                        let info = msg
                            .info()
                            .map_err(|e| jetstream_err("failed to get message info", e))?;
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

        let mut messages = match collected {
            Ok(m) => m,
            Err(e) => return Ok(Err(e)),
        };
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
            Err(e) => return Ok(Err(stream_lookup_err(format!("stream '{stream_name}'"), e))),
        };

        // Attach only: `create_consumer` is create-or-update and would rewrite
        // an existing durable's filter subject and deliver policy.
        //
        // Spelled out rather than using `get_consumer`, whose `crate::Error`
        // return erases every kind: a timeout, an unreachable JetStream API and
        // a consumer that exists but is push-based all come back the same, and
        // reporting any of them as `not-found` invites the guest to recreate a
        // consumer that is very much still there.
        let info = match stream.consumer_info(&consumer).await {
            Ok(info) => info,
            Err(e) => return Ok(Err(consumer_lookup_err(&stream_name, &consumer, e))),
        };
        let config = match jetstream::consumer::pull::Config::try_from_consumer_config(
            info.config.clone(),
        ) {
            Ok(config) => config,
            Err(e) => {
                return Ok(Err(types::NatsError::Jetstream(format!(
                    "consumer '{consumer}' on stream '{stream_name}' exists but is not a pull consumer: {e}"
                ))));
            }
        };

        let handle = PullConsumerHandle {
            consumer: Some(jetstream::consumer::Consumer::new(
                config,
                info,
                conn.jetstream.clone(),
            )),
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
        // The acker is cloned out and the borrow dropped before the await, so a
        // settle the wire rejected leaves the handle usable. Taking it up front
        // would turn the natural response to a reconnect blip — retry — into
        // "already settled", and the guest would conclude the ack landed while
        // the message was in fact about to redeliver.
        let h = self.table.get(&rep)?;
        let Some(acker) = h.acker.clone() else {
            return Ok(Err(already_settled()));
        };
        let settled = h.settled.clone();
        match acker.ack().await {
            Ok(()) => {
                settled.store(true, Ordering::Release);
                self.table.get_mut(&rep)?.acker.take();
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(jetstream_err("ack failed", e))),
        }
    }

    async fn nak(
        &mut self,
        rep: Resource<MessageHandle>,
        delay_ms: Option<u32>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let h = self.table.get(&rep)?;
        let Some(acker) = h.acker.clone() else {
            return Ok(Err(already_settled()));
        };
        let kind = jetstream::AckKind::Nak(delay_ms.map(|ms| Duration::from_millis(ms as u64)));
        match acker.ack_with(kind).await {
            Ok(()) => {
                self.table.get_mut(&rep)?.acker.take();
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(jetstream_err("nak failed", e))),
        }
    }

    async fn in_progress(
        &mut self,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        // Extending ack-wait settles nothing, so it reads the acker that is
        // present in both ack modes rather than the one that carries settlement
        // ownership. Keyed off the latter, the WIT-sanctioned way for a slow
        // handler to hold onto a message was unavailable under `ack-mode: auto`
        // — the default — and the handler was redelivered underneath itself.
        let h = self.table.get(&rep)?;
        let Some(progress) = h.progress.as_ref() else {
            return Ok(Err(already_settled()));
        };
        match progress.ack_with(jetstream::AckKind::Progress).await {
            Ok(()) => Ok(Ok(())),
            Err(e) => Ok(Err(jetstream_err("in-progress failed", e))),
        }
    }

    async fn term(
        &mut self,
        rep: Resource<MessageHandle>,
    ) -> wasmtime::Result<Result<(), types::NatsError>> {
        let h = self.table.get(&rep)?;
        let Some(acker) = h.acker.clone() else {
            return Ok(Err(already_settled()));
        };
        // A term is the guest saying the message must not come round again, so
        // it retires the sequence just as an ack does. A term the server never
        // took retires nothing, and has to stay retryable or a poison message
        // redelivers forever with no way left to discard it.
        let settled = h.settled.clone();
        match acker.ack_with(jetstream::AckKind::Term).await {
            Ok(()) => {
                settled.store(true, Ordering::Release);
                self.table.get_mut(&rep)?.acker.take();
                Ok(Ok(()))
            }
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

        // Ahead of the builder, so no `batch: 0` pull request reaches the
        // server. async-nats would end that stream empty, and the resulting
        // `no-messages` reads as a drained queue — sending a guest whose batch
        // size fell to zero home while its consumer is still backed up.
        if batch == 0 {
            return Ok(Err(types::NatsError::Unexpected(
                "fetch batch must be >= 1 (got 0)".to_string(),
            )));
        }

        let fetch = consumer
            .fetch()
            .max_messages(batch as usize)
            .expires(Duration::from_millis(timeout_ms as u64));
        let mut stream = match fetch.messages().await {
            Ok(s) => s,
            Err(e) => return Ok(Err(jetstream_err("fetch failed", e))),
        };

        let mut handles = Vec::new();
        // Messages that will not be handed to the guest because the resource
        // table filled up. A full table is a guest leaking handles, which every
        // sibling failure here reports as a typed error rather than killing the
        // instance for — so the batch is given back instead, and the server
        // redelivers it at once rather than holding it until ack-wait expires.
        let mut orphans: Vec<Arc<jetstream::message::Acker>> = Vec::new();
        let mut table_full = false;
        while let Some(next) = stream.next().await {
            // Already rolling back: keep draining so the tail of the batch is
            // handed back with the rest instead of sitting on the consumer.
            if table_full {
                if let Ok(msg) = next {
                    orphans.push(Arc::new(msg.split().1));
                }
                continue;
            }
            match next {
                Ok(msg) => {
                    let (sequence, delivery_count) = match msg.info() {
                        Ok(info) => (info.stream_sequence, info.delivered as u32),
                        Err(_) => (0, 1),
                    };
                    // Pull consumers are always guest-driven, so the acker
                    // goes to the handle regardless of ack-mode.
                    let (message, acker) = msg.split();
                    let acker = Arc::new(acker);
                    let pushed = self.table.push(MessageHandle {
                        acker: Some(acker.clone()),
                        progress: Some(acker.clone()),
                        settled: Arc::new(AtomicBool::new(false)),
                        message,
                        sequence,
                        delivery_count,
                    });
                    match pushed {
                        Ok(id) => handles.push(id),
                        Err(_) => {
                            table_full = true;
                            orphans.push(acker);
                            for id in std::mem::take(&mut handles) {
                                if let Ok(h) = self.table.delete(id)
                                    && let Some(a) = h.acker
                                {
                                    orphans.push(a);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    // A request the server refused outright — a batch over the
                    // consumer's `max-request-batch`, say — delivers nothing,
                    // and reporting that as `no-messages` would read as an idle
                    // consumer and send the guest into a retry loop it cannot
                    // win. `@0.2.0` types this as `limit-exceeded`; here it is
                    // at least an error with the server's own wording.
                    let detail = e.to_string();
                    match super::async_p3::classify_pull_end(&detail) {
                        super::async_p3::PullEnd::Refused => {
                            return Ok(Err(jetstream_err("fetch refused", detail)));
                        }
                        // What was already delivered is real and worth
                        // returning; only an empty batch has to carry the
                        // reason as an error, since nothing else can.
                        super::async_p3::PullEnd::Gone if handles.is_empty() => {
                            return Ok(Err(types::NatsError::NotFound(detail)));
                        }
                        super::async_p3::PullEnd::Interrupted if handles.is_empty() => {
                            return Ok(Err(jetstream_err("pull request was stood down", detail)));
                        }
                        _ => {}
                    }
                    warn!("pull-consumer fetch stream error: {e}");
                    break;
                }
            }
        }

        if table_full {
            for acker in orphans {
                if let Err(e) = acker.ack_with(jetstream::AckKind::Nak(None)).await {
                    warn!("failed to nak an unreturned fetched message: {e}");
                }
            }
            return Ok(Err(types::NatsError::Unexpected(
                "host resource table full".to_string(),
            )));
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
            Err(e) => return Ok(Err(bucket_lookup_err(&bucket, e))),
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
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::kv::EntryErrorKind::TimedOut);
                Ok(Err(kv_err("kv get failed", timed_out, e)))
            }
        }
    }

    async fn put(
        &mut self,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        // The same oversize body is `max-payload-exceeded` on publish, so it
        // has to be that here too — otherwise a guest that switches to chunked
        // storage on the typed variant instead reads a transient JetStream
        // fault and retries a write that can never land.
        let conn = conn_or_return!(self);
        if let Err(e) = check_payload(value.len(), None, &conn) {
            return Ok(Err(e));
        }
        let h = self.table.get(&rep)?;
        match h.store.put(&key, value.into()).await {
            Ok(rev) => Ok(Ok(rev)),
            // `PutError` folds the timeout into an opaque publish stage, so the
            // kind has to be walked to rather than read off the top.
            Err(e) => Ok(Err(kv_err("kv put failed", chain_timed_out(&e), e))),
        }
    }

    async fn create(
        &mut self,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        let conn = conn_or_return!(self);
        if let Err(e) = check_payload(value.len(), None, &conn) {
            return Ok(Err(e));
        }
        let h = self.table.get(&rep)?;
        match h.store.create(&key, value.into()).await {
            Ok(rev) => Ok(Ok(rev)),
            Err(e) => Ok(Err(kv_err("kv create failed", chain_timed_out(&e), e))),
        }
    }

    async fn update(
        &mut self,
        rep: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
        expected_revision: u64,
    ) -> wasmtime::Result<Result<u64, types::NatsError>> {
        let conn = conn_or_return!(self);
        if let Err(e) = check_payload(value.len(), None, &conn) {
            return Ok(Err(e));
        }
        let h = self.table.get(&rep)?;
        match h.store.update(&key, value.into(), expected_revision).await {
            Ok(rev) => Ok(Ok(rev)),
            Err(e) => {
                let msg = e.to_string();
                let mismatch =
                    matches!(e.kind(), jetstream::kv::UpdateErrorKind::WrongLastRevision)
                        || msg.to_ascii_lowercase().contains("wrong last sequence");
                if mismatch {
                    // The rejection already names the sequence the guest lost
                    // to ("wrong last sequence: N"), so read it from there:
                    // that keeps the promised revision available even on a
                    // connection too degraded to answer a second round-trip.
                    if let Some(actual) = msg
                        .rsplit(':')
                        .next()
                        .and_then(|s| s.trim().parse::<u64>().ok())
                    {
                        return Ok(Err(types::NatsError::RevisionMismatch(actual)));
                    }
                    // Only when the wording changes: re-read the key. If that
                    // fails too, report the original failure — `revision 0` is
                    // not a neutral placeholder but an instruction to blind-
                    // create, and a guest retrying against it loops forever.
                    return Ok(Err(match h.store.entry(&key).await {
                        Ok(Some(entry)) => types::NatsError::RevisionMismatch(entry.revision),
                        Ok(None) => types::NatsError::RevisionMismatch(0),
                        Err(_) => jetstream_err("kv update failed", e),
                    }));
                }
                let timed_out = matches!(e.kind(), jetstream::kv::UpdateErrorKind::TimedOut);
                Ok(Err(kv_err("kv update failed", timed_out, e)))
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
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::kv::DeleteErrorKind::TimedOut);
                Ok(Err(kv_err("kv delete failed", timed_out, e)))
            }
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
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::kv::PurgeErrorKind::TimedOut);
                Ok(Err(kv_err("kv purge failed", timed_out, e)))
            }
        }
    }

    async fn keys(
        &mut self,
        rep: Resource<BucketHandle>,
    ) -> wasmtime::Result<Result<Vec<String>, types::NatsError>> {
        let h = self.table.get(&rep)?;
        let mut iter = match h.store.keys().await {
            Ok(i) => i,
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::kv::WatchErrorKind::TimedOut);
                return Ok(Err(kv_err("kv keys failed", timed_out, e)));
            }
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
                Err(e) => {
                    return Ok(Err(kv_err("kv keys iter failed", chain_timed_out(&e), e)));
                }
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
        // The history stream ends when the server reports nothing pending, and
        // a subject holding zero messages never gets that far: the ordered
        // consumer simply delivers nothing and the drain below would wedge the
        // whole guest call. `entry` returns `Ok(None)` for exactly that case —
        // tombstones still come back as `Some` and terminate normally.
        match h.store.entry(&key).await {
            Ok(Some(_)) => {}
            // This revision has no key-not-found on history, and an empty list
            // is the honest answer for a key with nothing recorded.
            Ok(None) => return Ok(Ok(Vec::new())),
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::kv::EntryErrorKind::TimedOut);
                return Ok(Err(kv_err("kv history failed", timed_out, e)));
            }
        }

        let mut hist = match h.store.history(&key).await {
            Ok(h) => h,
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::kv::WatchErrorKind::TimedOut);
                return Ok(Err(kv_err("kv history failed", timed_out, e)));
            }
        };
        let mut out = Vec::new();
        // The probe above closes the ordinary empty case; the deadline covers
        // the history expiring in the gap between it and the consumer, and any
        // server that simply stops answering.
        let deadline = tokio::time::Instant::now() + MAX_HISTORY_DURATION;
        loop {
            match tokio::time::timeout_at(deadline, hist.next()).await {
                Err(_) => {
                    return Ok(Err(types::NatsError::Timeout(
                        "kv history did not complete within 10s".to_string(),
                    )));
                }
                Ok(None) => break,
                Ok(Some(Ok(e))) => out.push(kv_entry_to_wit(&e)),
                Ok(Some(Err(e))) => {
                    return Ok(Err(kv_err(
                        "kv history iter failed",
                        chain_timed_out(&e),
                        e,
                    )));
                }
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
            Err(e) => {
                let timed_out = matches!(e.kind(), jetstream::context::RequestErrorKind::TimedOut);
                return Ok(Err(kv_err("kv status failed", timed_out, e)));
            }
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
