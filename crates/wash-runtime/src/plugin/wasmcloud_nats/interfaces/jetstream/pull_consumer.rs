//! `wasmcloud:nats/jetstream@0.1.0#pull-consumer` — guest-driven batch pulls
//! off an existing durable consumer.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use async_nats::jetstream;
use futures::StreamExt as _;
use tracing::{debug, warn};
use wasmtime::component::{Accessor, Resource};

use crate::engine::ctx::{ActiveCtx, SharedCtx};

use super::consumer_info_to_wit;
use crate::plugin::wasmcloud_nats::interfaces::{consumer_lookup_err, jetstream_err, js, types};
use crate::plugin::wasmcloud_nats::jetstream::{FetchBudget, MessageHandle, PullConsumerHandle};

/// Clones the consumer out of its resource, so the table borrow is not held
/// across an await — `ActiveCtx` is not `Send`.
fn consumer_ref<T>(
    access: &mut wasmtime::component::Access<'_, T, SharedCtx>,
    rep: &Resource<PullConsumerHandle>,
) -> wasmtime::Result<Option<jetstream::consumer::Consumer<jetstream::consumer::pull::Config>>> {
    Ok(access.get().table.get(rep)?.consumer.clone())
}

/// The binding's ceiling on one fetch, for the variant that names no bytes.
fn max_fetch_bytes<T>(
    access: &mut wasmtime::component::Access<'_, T, SharedCtx>,
    rep: &Resource<PullConsumerHandle>,
) -> wasmtime::Result<u64> {
    Ok(access.get().table.get(rep)?.max_fetch_bytes)
}

/// The binding-wide budget this consumer charges against.
fn fetch_budget<T>(
    access: &mut wasmtime::component::Access<'_, T, SharedCtx>,
    rep: &Resource<PullConsumerHandle>,
) -> wasmtime::Result<Arc<FetchBudget>> {
    Ok(access.get().table.get(rep)?.budget.clone())
}

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

/// The shared body of both `fetch` variants. `max_bytes` of 0 means the guest
/// named no byte bound, and the binding's stands in for one.
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
    let budget = accessor.with(|mut a| fetch_budget(&mut a, &rep))?;
    let binding_bound = accessor.with(|mut a| max_fetch_bytes(&mut a, &rep))?;

    // Bounding one call bounds one call. What OOM-killed the host is the
    // ordinary shape of a pull worker — loop `fetch` until drained — because a
    // delivered message stays resident until its handle is dropped, and acking
    // does not drop it. So the byte bound this fetch is given is what the
    // binding has *left*, not what it started with.
    let available = budget.available();
    if available == 0 {
        return Ok(Err(types::NatsError::LimitExceeded(format!(
            "this binding already holds {held} bytes of fetched messages, its whole \
             `subscription-capacity-bytes` budget of {ceiling}. Drop the message handles \
             from earlier batches — acknowledging one does not release it — or raise \
             `subscription-capacity-bytes`.",
            held = budget.outstanding(),
            ceiling = budget.ceiling(),
        ))));
    }
    let effective_bytes = match max_bytes {
        // The guest named no bound, so the host's own stands in — and a bound
        // the host chose must not push the request past one the consumer would
        // have honoured. The server refuses an over-`max-request-max-bytes`
        // pull outright rather than trimming it, so a binding whose ceiling
        // sits above the consumer's would turn every plain `fetch` into a
        // refusal the guest never asked for.
        0 => {
            let ours = binding_bound.min(available);
            match consumer.cached_info().config.max_bytes {
                limit if limit > 0 => ours.min(limit as u64),
                _ => ours,
            }
        }
        // The guest named one. Over the consumer's limit the server refuses,
        // and that refusal is the guest's to see — the same answer an
        // over-`max-request-batch` ask gets.
        asked => asked.min(available),
    };

    let mut fetch = consumer
        .fetch()
        .max_messages(batch as usize)
        .expires(Duration::from_millis(timeout_ms as u64));
    fetch = fetch.max_bytes(effective_bytes.min(usize::MAX as u64) as usize);
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
                // Charged here, released by `MessageHandle::drop` — the point
                // at which the payload is actually freed.
                let charged = message.length as u64;
                budget.charge(charged);
                fetched.push(MessageHandle {
                    acker: Some(acker.clone()),
                    progress: Some(acker),
                    settled: Arc::new(AtomicBool::new(false)),
                    message,
                    sequence,
                    delivery_count,
                    charged: Some((budget.clone(), charged)),
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
            Some(PullEnd::ByteLimit) => Ok(Err(types::NatsError::LimitExceeded(format!(
                "the next message is larger than this fetch's {effective_bytes}-byte bound; \
                 `fetch` without one is bound by what is left of the binding's \
                 `subscription-capacity-bytes` after the handles it still holds"
            )))),
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
        // The guest named a count and no byte bound, so the binding's stands in
        // for one: unbounded, `fetch(100)` on a jumbo stream materializes the
        // whole batch in host memory and takes the host down with it. A batch
        // the bound cuts short reports `byte-limit`, which is already how the
        // guest learns there is more to come.
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
