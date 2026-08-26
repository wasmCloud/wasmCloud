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
//!
//! ## Layout
//!
//! This module holds the generated bindings, the helpers the interfaces share,
//! and `core` itself — small enough to sit beside them. Everything else is a
//! module of its own:
//!
//! - [`jetstream`], with a submodule per resource ([`jetstream::message_handle`],
//!   [`jetstream::pull_consumer`]) and one for the JetStream-backed
//!   [`jetstream::kv`] interface.
//! - [`default`], the plain unlabeled route.
//! - [`labeled`], the label-routed resource methods.
//!
//! The last two are the two halves of the routing story: a function is
//! implemented once for the label-routed interface and delegated to from the
//! plain one, while a resource method — which needs no routing, since the
//! resource carries its connection — goes the other way.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tracing::{debug, warn};
use wasmtime::component::Accessor;

use crate::engine::ctx::{ActiveCtx, SharedCtx};

use super::conn::ConnHandle;
use super::policy::Denied;
use super::{PLUGIN_NATS_ID, WasmcloudNats};

mod default;
mod jetstream;
mod labeled;

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
            "wasmcloud:nats/jetstream@0.1.0.message-handle": super::super::jetstream::MessageHandle,
            "wasmcloud:nats/jetstream@0.1.0.pull-consumer": super::super::jetstream::PullConsumerHandle,
            "wasmcloud:nats/kv@0.1.0.bucket": super::super::jetstream::BucketHandle,
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
        match crate::plugin::wasmcloud_nats::interfaces::conn($accessor).await {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        }
    };
}
pub(super) use conn_or_return;

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
super::macros::nats_error_classifiers!();

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
