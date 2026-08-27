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
//! This module holds the generated bindings and `core` itself — small enough to
//! sit beside them. Everything else is a module of its own:
//!
//! - [`helpers`], the connection lookup, policy checks and conversions every
//!   interface shares.
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
use tracing::debug;
use wasmtime::component::Accessor;

use crate::engine::ctx::{ActiveCtx, SharedCtx};

use super::conn::ConnHandle;
use super::ledger;

mod default;
mod helpers;
mod jetstream;
mod labeled;

pub(super) mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "wasmcloud:nats/imports@0.1.0",
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
use helpers::*;
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
        // Every refusal below is counted, not only logged. A core publish is
        // fire-and-forget, so a message that never reaches the wire is
        // invisible to the guest, to the receiver, and — until this — to the
        // operator comparing what a fan-out published against what arrived.
        let size = body.len();
        if let Err(e) = check_publish_subject(&conn, &subject) {
            conn.publishes
                .dropped(&subject, size, ledger::PublishDrop::Refused);
            return Ok(Err(e));
        }
        // The reply-to the guest asks responses on is a subject it is inviting
        // traffic to, not one the host handed it, so it gets no carve-out.
        if let Some(reply_to) = reply_to.as_deref()
            && let Err(e) = check_subject(&conn, reply_to)
        {
            conn.publishes
                .dropped(&subject, size, ledger::PublishDrop::Refused);
            return Ok(Err(e));
        }
        let headers = match outbound_headers(headers.as_deref()) {
            Ok(h) => h,
            Err(e) => {
                conn.publishes
                    .dropped(&subject, size, ledger::PublishDrop::Refused);
                return Ok(Err(e));
            }
        };
        if let Err(e) = check_payload(body.len(), headers.as_ref(), &conn) {
            conn.publishes
                .dropped(&subject, size, ledger::PublishDrop::Refused);
            return Ok(Err(e));
        }

        let payload: Bytes = body.into();
        let subject_label = subject.clone();
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

        match &result {
            // The wire is as far as a core publish can be followed. What this
            // counter buys is the *other* side of the comparison: a receiver
            // that saw fewer than this published them and lost them
            // downstream, which is a different problem from never having
            // published them and was previously indistinguishable from it.
            Ok(()) => conn.publishes.published(size),
            Err(_) => conn
                .publishes
                .dropped(&subject_label, size, ledger::PublishDrop::Failed),
        }
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
