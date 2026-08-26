//! `wasmcloud:nats/jetstream@0.1.0#message-handle` — one delivered message and
//! the settlement operations over it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_nats::jetstream;
use wasmtime::component::{Accessor, Resource};

use crate::engine::ctx::{ActiveCtx, SharedCtx};

use crate::plugin::wasmcloud_nats::handles::MessageHandle;
use crate::plugin::wasmcloud_nats::interfaces::{build_nats_message, jetstream_err, js, types};

/// Reported when a guest settles a message the host already owns.
fn already_settled() -> types::NatsError {
    types::NatsError::Unexpected(
        "message already settled, or acknowledgement is owned by the host under ack-mode: auto"
            .to_string(),
    )
}

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
