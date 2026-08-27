//! Helpers shared by every interface in the package: connection lookup, the
//! policy checks each call runs before it touches the wire, header and payload
//! conversion, and the error classifiers.

use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, warn};
use wasmtime::component::Accessor;

use crate::engine::ctx::SharedCtx;

use super::super::conn::ConnHandle;
use super::super::policy::Denied;
use super::super::{PLUGIN_NATS_ID, WasmcloudNats};
use super::types;

/// Resolves the calling workload's connection.
///
/// The context borrow is released before the await: `ActiveCtx` is not `Send`.
pub(super) async fn conn<T: 'static + Send>(
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
        match crate::plugin::wasmcloud_nats::interfaces::conn($accessor).await {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        }
    };
}
pub(super) use conn_or_return;

/// Lowers a policy refusal onto the wire, keeping the reason and the kind of
/// name that was refused — `@0.1.0` collapsed both into one string.
pub(super) fn denied(
    reason: Denied,
    target: types::DeniedResource,
    name: &str,
) -> types::NatsError {
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
pub(super) fn check_subject(conn: &ConnHandle, subject: &str) -> Result<(), types::NatsError> {
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
pub(super) fn check_publish_subject(
    conn: &ConnHandle,
    subject: &str,
) -> Result<(), types::NatsError> {
    match conn.policy.check_subject(subject) {
        Ok(()) => Ok(()),
        Err(Denied::NotGranted) if conn.take_reply_grant(subject) => Ok(()),
        Err(reason) => Err(denied(reason, types::DeniedResource::Subject, subject)),
    }
}

pub(super) fn check_stream(conn: &ConnHandle, stream: &str) -> Result<(), types::NatsError> {
    conn.policy
        .check_stream(stream)
        .map_err(|reason| denied(reason, types::DeniedResource::Stream, stream))
}

pub(super) fn check_bucket(conn: &ConnHandle, bucket: &str) -> Result<(), types::NatsError> {
    conn.policy
        .check_bucket(bucket)
        .map_err(|reason| denied(reason, types::DeniedResource::Bucket, bucket))
}

pub(super) fn jetstream_err(
    ctx: impl std::fmt::Display,
    e: impl std::fmt::Display,
) -> types::NatsError {
    types::NatsError::Jetstream(format!("{ctx}: {e}"))
}

// The shared error classifiers, over this world's generated `types`.
super::super::macros::nats_error_classifiers!();

/// Converts guest headers, rejecting anything async-nats would assert on.
pub(super) fn wit_headers_to_nats(
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
pub(super) fn outbound_headers(
    headers: Option<&[types::HeaderEntry]>,
) -> Result<Option<async_nats::HeaderMap>, types::NatsError> {
    match headers.filter(|h| !h.is_empty()) {
        Some(h) => wit_headers_to_nats(h).map(Some),
        None => Ok(None),
    }
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

/// Serialized size of a header block, mirroring what async-nats puts on the
/// wire. Its own `wire_len` is crate-private.
pub(super) fn headers_wire_len(headers: &async_nats::HeaderMap) -> usize {
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
pub(super) fn check_payload(
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

pub(super) fn build_nats_message(
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

/// How long a collecting JetStream call may run before it is reported as
/// slow, when the binding names no `request-timeout-ms`.
pub(super) const SLOW_CALL_THRESHOLD: Duration = Duration::from_secs(2);

/// Runs a host call that opens a stream rather than making one request, and
/// leaves a trace either way.
///
/// `client.request` is bounded by `request-timeout-ms` and JetStream's own API
/// calls by the context timeout, but a call that *drains* something — a KV
/// history, a paged subject walk — is a sequence of those with nothing
/// bounding the sequence. Two of them were reported as producing no receipt,
/// no error and no log line at all, which is the worst possible failure to
/// diagnose: it is indistinguishable from the guest never having called.
///
/// So every such call now ends in one of three states, all of them visible:
///
/// - completed promptly — `debug`, with how long it took;
/// - completed slowly — `warn`, because a call that takes seconds is a fault
///   even when it eventually answers;
/// - did not complete — `warn`, and a typed [`types::NatsError::Timeout`] the
///   guest can act on rather than an await that never returns.
///
/// The deadline is the binding's `request-timeout-ms` where it has one, since
/// that is the operator's own statement about how long a call may take, and
/// `fallback` otherwise.
pub(super) async fn with_deadline<T>(
    conn: &ConnHandle,
    call: &'static str,
    detail: &str,
    fallback: Duration,
    fut: impl std::future::Future<Output = T>,
) -> Result<T, types::NatsError> {
    let limit = conn.request_timeout.unwrap_or(fallback);
    let started = tokio::time::Instant::now();
    match tokio::time::timeout(limit, fut).await {
        Ok(value) => {
            let elapsed = started.elapsed();
            if elapsed >= SLOW_CALL_THRESHOLD.min(limit) {
                tracing::warn!(
                    call,
                    detail,
                    elapsed_ms = elapsed.as_millis() as u64,
                    limit_ms = limit.as_millis() as u64,
                    "wasmcloud:nats call was slow to complete"
                );
            } else {
                debug!(
                    call,
                    detail,
                    elapsed_ms = elapsed.as_millis() as u64,
                    "wasmcloud:nats call completed"
                );
            }
            Ok(value)
        }
        Err(_) => {
            tracing::warn!(
                call,
                detail,
                limit_ms = limit.as_millis() as u64,
                "wasmcloud:nats call did not complete within its deadline; returning a timeout \
                 rather than blocking the guest. Raise `request-timeout-ms` if the call is \
                 legitimately this slow."
            );
            Err(types::NatsError::Timeout(format!(
                "{call} on {detail} did not complete within {}ms",
                limit.as_millis()
            )))
        }
    }
}
