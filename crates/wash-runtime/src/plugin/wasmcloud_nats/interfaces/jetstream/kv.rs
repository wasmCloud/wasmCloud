//! `wasmcloud:nats/kv@0.1.0` — the JetStream KV store, and the `bucket`
//! resource an `open` hands back.

use core::future::Future as _;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::consumer::pull::MessagesError;
use futures::StreamExt as _;
use futures::stream::BoxStream;
use tokio::sync::oneshot;
use tracing::{debug, warn};
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Accessor, Destination, FutureReader, Resource, StreamProducer, StreamReader, StreamResult,
    VecBuffer,
};

use crate::engine::ctx::{ActiveCtx, SharedCtx};

use crate::plugin::wasmcloud_nats::interfaces::{
    NatsId, bucket_lookup_err, chain_timed_out, check_bucket, check_payload, jetstream_err, kv,
    kv_err, labeled_kv, types, with_deadline,
};
use crate::plugin::wasmcloud_nats::jetstream::BucketHandle;

/// Wall-clock bound on one `history` drain, for the same reason `scan` has one.
const MAX_HISTORY_DURATION: Duration = Duration::from_secs(10);
/// Cap on keys returned by one `keys` call.
const KV_KEYS_BATCH: usize = 1000;
/// Entries one `select` poll will gather before handing them to the guest.
/// Bounds host memory per poll; the drain as a whole stays unbounded.
const DEFAULT_SELECT_BATCH: usize = 1024;

/// Refuses a `keys` filter that could not be a KV subject pattern.
///
/// It is concatenated onto the bucket's subject prefix, so an empty token or
/// stray whitespace produces a filter the server rejects at consumer-create
/// time, with a message that names neither the bucket nor the call.
fn validate_key_filter(filter: &str) -> Result<(), types::NatsError> {
    let bad = filter.is_empty()
        || filter.chars().any(char::is_whitespace)
        || filter.split('.').any(str::is_empty);
    if bad {
        return Err(types::NatsError::Unexpected(format!(
            "kv keys filter `{filter}` is not a valid subject filter; use `>` for every key"
        )));
    }
    Ok(())
}

/// The KV operation a raw stream message represents. The op rides in a
/// header; anything else is a put. `kv_entry_to_wit` reads the same thing off
/// a decoded `jetstream::kv::Entry`, which `select` does not have -- it reads
/// the consumer directly.
fn kv_operation(message: &async_nats::Message) -> kv::KvOperation {
    match message
        .headers
        .as_ref()
        .and_then(|h| h.get("KV-Operation"))
        .map(|op| op.as_str())
    {
        Some("DEL") => kv::KvOperation::Delete,
        Some("PURGE") => kv::KvOperation::Purge,
        _ => kv::KvOperation::Put,
    }
}

/// Whether a KV message is a delete or purge tombstone rather than a live
/// value. The operation rides in a header; anything else is a put.
fn is_tombstone(message: &async_nats::Message) -> bool {
    message
        .headers
        .as_ref()
        .and_then(|h| h.get("KV-Operation"))
        .is_some_and(|op| {
            let op = op.as_str();
            op == "DEL" || op == "PURGE"
        })
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

/// Clones the bucket's store out of its resource, for the same reason.
fn store_ref<T>(
    access: &mut wasmtime::component::Access<'_, T, SharedCtx>,
    rep: &Resource<BucketHandle>,
) -> wasmtime::Result<jetstream::kv::Store> {
    Ok(access.get().table.get(rep)?.store.clone())
}

/// The connection the bucket was opened on, for the checks a write runs.
///
/// Not `conn()`: that resolves the *unnamed* binding by workload id, which a
/// labeled-only import does not have and a labeled import does not want.
fn conn_ref<T>(
    access: &mut wasmtime::component::Access<'_, T, SharedCtx>,
    rep: &Resource<BucketHandle>,
) -> wasmtime::Result<std::sync::Arc<crate::plugin::wasmcloud_nats::conn::ConnHandle>> {
    Ok(access.get().table.get(rep)?.conn.clone())
}

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
        let resource = accessor.with(|mut a| a.get().table.push(BucketHandle { store, conn }))?;
        Ok(Ok(resource))
    }
}

impl kv::Host for ActiveCtx<'_> {}

/// Streams live KV entries off one no-ack `last-per-subject` pull consumer.
///
/// Values ride along instead of costing a `get()` each. At most
/// `DEFAULT_SELECT_BATCH` entries are held per poll, and the guest's read rate
/// is the flow control, so an unbounded drain does not grow host memory.
struct KvSelectProducer {
    messages: BoxStream<'static, Result<jetstream::Message, MessagesError>>,
    prefix: String,
    include_tombstones: bool,
    include_values: bool,
    /// Caller's own ceiling. 0 is unbounded — the host imposes none here.
    max_entries: u64,
    emitted: u64,
    /// Messages the consumer reported pending when it was created.
    ///
    /// This is the deterministic end-of-drain signal. Relying only on a
    /// message's `pending == 0` is not reliable: when that observation is
    /// missed the stream stops yielding and the drain blocks until the
    /// consumer times out, which showed up as a 40k read taking 10.4s
    /// instead of 350ms — the same read, quantised to the timeout.
    expected: u64,
    /// Messages taken off the consumer, tombstones included. `emitted` counts
    /// only what reached the guest, so it undercounts against `expected`
    /// whenever a tombstone is skipped.
    consumed: u64,
    /// Resolves once, carrying the terminal status. A stream that ends
    /// because the consumer went away must not read as a completed drain, so
    /// this is the only thing that says which happened.
    result: Option<oneshot::Sender<Result<(), types::NatsError>>>,
    finished: bool,
    /// Wall-clock bound on the whole drain, and the limit it was built from.
    /// `None` is unbounded.
    deadline: Option<(Pin<Box<tokio::time::Sleep>>, Duration)>,
}

/// The drain deadline: the caller's `timeout-ms`, else the binding's
/// `request-timeout-ms`, else none.
fn select_timeout(timeout_ms: u32, request_timeout: Option<Duration>) -> Option<Duration> {
    match timeout_ms {
        0 => request_timeout,
        ms => Some(Duration::from_millis(ms.into())),
    }
}

impl KvSelectProducer {
    /// Reports the terminal status exactly once; later calls are no-ops.
    fn finish(&mut self, outcome: Result<(), types::NatsError>) {
        debug!(
            expected = self.expected,
            consumed = self.consumed,
            emitted = self.emitted,
            ok = outcome.is_ok(),
            "kv select drain finished"
        );
        if let Some(tx) = self.result.take() {
            let _ = tx.send(outcome);
        }
        self.finished = true;
    }
}

impl<D> StreamProducer<D> for KvSelectProducer
where
    D: 'static,
{
    type Item = kv::Entry;
    /// A vector, not `Option<Entry>`: handing entries over one per
    /// `poll_produce` costs a full round trip through the stream machinery
    /// per entry, which is what made a 40k drain take 11s while 10k took
    /// 234ms. Batching makes the cost linear again.
    type Buffer = VecBuffer<Self::Item>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        mut dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if self.finished {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        // Polled first so the waker is registered even when we go Pending below.
        if let Some((sleep, limit)) = self.deadline.as_mut()
            && sleep.as_mut().poll(cx).is_ready()
        {
            let limit_ms = limit.as_millis();
            warn!(
                limit_ms,
                emitted = self.emitted,
                "kv select drain hit its deadline"
            );
            self.finish(Err(types::NatsError::Timeout(format!(
                "kv select did not complete within {limit_ms}ms"
            ))));
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        if dst.remaining(&mut store) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        if self.max_entries != 0 && self.emitted >= self.max_entries {
            self.finish(Ok(()));
            return Poll::Ready(Ok(StreamResult::Dropped));
        }

        // Fill up to whatever the current read can take, bounded so a huge
        // read window cannot make one poll unbounded.
        let cap = dst
            .remaining(&mut store)
            .unwrap_or(DEFAULT_SELECT_BATCH)
            .min(DEFAULT_SELECT_BATCH);
        let mut batch: Vec<kv::Entry> = Vec::with_capacity(cap.min(1024));
        let mut ended = false;
        let mut failure: Option<types::NatsError> = None;

        while batch.len() < cap {
            if self.max_entries != 0 && self.emitted + batch.len() as u64 >= self.max_entries {
                ended = true;
                break;
            }
            let next = match self.messages.poll_next_unpin(cx) {
                Poll::Ready(next) => next,
                Poll::Pending => {
                    if batch.is_empty() {
                        // Nothing to hand over yet. `finish` means the guest
                        // is going away, not that the drain completed --
                        // report it as cancelled, never as clean.
                        if finish {
                            return Poll::Ready(Ok(StreamResult::Cancelled));
                        }
                        return Poll::Pending;
                    }
                    // Hand over what we have rather than holding it back.
                    break;
                }
            };
            let message = match next {
                Some(Ok(m)) => m,
                Some(Err(e)) => {
                    let timed_out = chain_timed_out(&e);
                    failure = Some(kv_err("kv select iter failed", timed_out, e));
                    break;
                }
                None => {
                    ended = true;
                    break;
                }
            };

            self.consumed += 1;
            let last = message.info().map(|info| info.pending == 0).unwrap_or(true)
                || (self.expected != 0 && self.consumed >= self.expected);
            let tombstone = is_tombstone(&message.message);
            if (!tombstone || self.include_tombstones)
                && let Some(key) = message
                    .subject
                    .strip_prefix(self.prefix.as_str())
                    .map(str::to_string)
            {
                batch.push(kv::Entry {
                    key,
                    value: if self.include_values {
                        message.payload.to_vec()
                    } else {
                        Vec::new()
                    },
                    revision: message.info().map(|i| i.stream_sequence).unwrap_or(0),
                    created_at_unix_nanos: message
                        .info()
                        .map(|i| {
                            i.published
                                .unix_timestamp_nanos()
                                .try_into()
                                .unwrap_or_default()
                        })
                        .unwrap_or(0),
                    operation: kv_operation(&message.message),
                });
            }
            if last {
                ended = true;
                break;
            }
        }

        self.emitted += batch.len() as u64;
        let empty = batch.is_empty();
        if !empty {
            dst.set_buffer(VecBuffer::from(batch));
        }

        if let Some(e) = failure {
            self.finish(Err(e));
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        if ended {
            self.finish(Ok(()));
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        Poll::Ready(Ok(StreamResult::Completed))
    }
}

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
        let (store, conn) = accessor.with(|mut a| {
            Ok::<_, wasmtime::Error>((store_ref(&mut a, &rep)?, conn_ref(&mut a, &rep)?))
        })?;
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
        let (store, conn) = accessor.with(|mut a| {
            Ok::<_, wasmtime::Error>((store_ref(&mut a, &rep)?, conn_ref(&mut a, &rep)?))
        })?;
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
        let (store, conn) = accessor.with(|mut a| {
            Ok::<_, wasmtime::Error>((store_ref(&mut a, &rep)?, conn_ref(&mut a, &rep)?))
        })?;
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
        filter: String,
    ) -> wasmtime::Result<Result<kv::KeyPage, types::NatsError>> {
        let (store, conn) = accessor.with(|mut a| {
            let handle = a.get().table.get(&rep)?;
            Ok::<_, wasmtime::Error>((handle.store.clone(), handle.conn.clone()))
        })?;
        if let Err(e) = validate_key_filter(&filter) {
            return Ok(Err(e));
        }
        // The filter goes onto the consumer rather than being applied to a
        // full listing: the cap then bounds *matched* keys, which is what
        // gives a guest a way to reach past it in a bucket that holds more.
        // `Store::keys` is this consumer with a filter of `>` hard-coded.
        let consumer = match store
            .stream
            .create_consumer(jetstream::consumer::push::OrderedConfig {
                deliver_subject: conn.client.new_inbox(),
                description: Some("wasmcloud:nats kv keys consumer".to_string()),
                filter_subject: format!("{}{filter}", store.prefix),
                headers_only: true,
                replay_policy: jetstream::consumer::ReplayPolicy::Instant,
                // Only the current state of each key, not its whole history.
                deliver_policy: jetstream::consumer::DeliverPolicy::LastPerSubject,
                ..Default::default()
            })
            .await
        {
            Ok(c) => c,
            Err(e) => return Ok(Err(jetstream_err("kv keys failed", e))),
        };
        // A filter matching nothing yields a consumer with nothing pending,
        // and its message stream would never end on its own.
        if consumer.cached_info().num_pending == 0 {
            return Ok(Ok(kv::KeyPage {
                keys: Vec::new(),
                truncated: false,
            }));
        }
        let mut messages = match consumer.messages().await {
            Ok(m) => m,
            Err(e) => return Ok(Err(jetstream_err("kv keys failed", e))),
        };

        let mut out = Vec::new();
        let mut truncated = false;
        while let Some(next) = messages.next().await {
            let message = match next {
                Ok(m) => m,
                Err(e) => {
                    let timed_out = chain_timed_out(&e);
                    return Ok(Err(kv_err("kv keys iter failed", timed_out, e)));
                }
            };
            let last = message.info().map(|info| info.pending == 0).unwrap_or(true);
            // A delete or purge tombstone is still the latest message on its
            // subject, so it arrives here and is not a live key.
            if !is_tombstone(&message) {
                if let Some(key) = message.subject.strip_prefix(store.prefix.as_str()) {
                    out.push(key.to_string());
                }
                // The cap stays — draining an arbitrarily large bucket into
                // one guest allocation is its own failure mode — but the walk
                // goes one key past it. That key is the only evidence the
                // bucket holds more, and dropping it is what made a partial
                // listing indistinguishable from a whole one.
                if out.len() > KV_KEYS_BATCH {
                    warn!(
                        %filter,
                        "kv keys truncated at {KV_KEYS_BATCH} entries — narrow the filter to \
                         reach the rest"
                    );
                    out.truncate(KV_KEYS_BATCH);
                    truncated = true;
                    break;
                }
            }
            if last {
                break;
            }
        }
        Ok(Ok(kv::KeyPage {
            keys: out,
            truncated,
        }))
    }

    async fn select(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        filter: String,
        opts: kv::SelectOptions,
    ) -> wasmtime::Result<
        Result<
            (
                StreamReader<kv::Entry>,
                FutureReader<Result<(), types::NatsError>>,
            ),
            types::NatsError,
        >,
    > {
        let (store, conn) = accessor.with(|mut a| {
            let handle = a.get().table.get(&rep)?;
            Ok::<_, wasmtime::Error>((handle.store.clone(), handle.conn.clone()))
        })?;
        if let Err(e) = validate_key_filter(&filter) {
            return Ok(Err(e));
        }

        // A *pull* consumer, deliberately -- `keys()` uses an ordered push
        // consumer, and that shape is wrong for a long drain.
        //
        // An ordered push consumer has no flow control: the server pushes as
        // fast as it can, and when the client cannot keep up messages are
        // dropped. async-nats notices the gap only via the 5s
        // `ORDERED_IDLE_HEARTBEAT` (doubled to 10s at push.rs:129), then
        // resets the consumer and resumes. That is exactly what a 40k drain
        // showed: identical, always-correct results arriving in
        // 400ms / 5.4s / 10.4s -- work plus N x 5s, with the drain itself
        // reporting expected == consumed == emitted every time. The stall was
        // never the end condition; it was mid-delivery heartbeat resets.
        //
        // A pull consumer asks for what it wants, so nothing is dropped and
        // no heartbeat is involved.
        let consumer = match store
            .stream
            .create_consumer(jetstream::consumer::pull::Config {
                description: Some("wasmcloud:nats kv select consumer".to_string()),
                filter_subject: format!("{}{filter}", store.prefix),
                headers_only: !opts.include_values,
                replay_policy: jetstream::consumer::ReplayPolicy::Instant,
                deliver_policy: jetstream::consumer::DeliverPolicy::LastPerSubject,
                // No acks. This is a read-only drain, and the default
                // `explicit` policy means the server stops delivering once
                // `max_ack_pending` (1000) messages are unacked -- which
                // showed up as a body truncated at 128 KiB and then a hang.
                ack_policy: jetstream::consumer::AckPolicy::None,
                // Reaped server-side if the host dies mid-drain, so an
                // abandoned consumer cannot count against `max_consumers`
                // forever. Well above any healthy drain.
                inactive_threshold: Duration::from_secs(60),
                ..Default::default()
            })
            .await
        {
            Ok(c) => c,
            Err(e) => return Ok(Err(jetstream_err("kv select failed", e))),
        };

        let (result_tx, result_rx) = oneshot::channel();
        // A filter matching nothing yields a consumer with nothing pending.
        // A pull consumer's message stream never ends on its own, so an empty
        // result has to be recognised here rather than waited for.
        let pending = consumer.cached_info().num_pending;
        let messages: BoxStream<'static, Result<jetstream::Message, MessagesError>> =
            if pending == 0 {
                futures::stream::empty().boxed()
            } else {
                match consumer.messages().await {
                    Ok(m) => m.boxed(),
                    Err(e) => return Ok(Err(jetstream_err("kv select failed", e))),
                }
            };

        let producer = KvSelectProducer {
            messages,
            expected: pending,
            consumed: 0,
            prefix: store.prefix.clone(),
            include_tombstones: opts.include_tombstones,
            include_values: opts.include_values,
            max_entries: opts.max_entries,
            emitted: 0,
            result: Some(result_tx),
            finished: false,
            deadline: select_timeout(opts.timeout_ms, conn.request_timeout)
                .map(|limit| (Box::pin(tokio::time::sleep(limit)), limit)),
        };

        debug!(%filter, include_values = opts.include_values, "kv select started");
        accessor.with(|mut store| {
            let stream = StreamReader::new(&mut store, producer)?;
            let future = FutureReader::new(&mut store, async move {
                wasmtime::error::Ok(result_rx.await.unwrap_or_else(|_| {
                    Err(types::NatsError::Unexpected(
                        "kv select ended without reporting a terminal status".to_string(),
                    ))
                }))
            })?;
            Ok(Ok((stream, future)))
        })
    }

    async fn history(
        accessor: &Accessor<T, Self>,
        rep: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<Vec<kv::Entry>, types::NatsError>> {
        let (store, conn) = accessor.with(|mut a| {
            let handle = a.get().table.get(&rep)?;
            Ok::<_, wasmtime::Error>((handle.store.clone(), handle.conn.clone()))
        })?;
        // Every await below is wrapped, and none of them were. This call was
        // reported as producing no receipt, no error and no log line — a state
        // indistinguishable from the guest never having called at all — so the
        // point is as much that it now always leaves a trace as that it now
        // always returns. See `with_deadline`.
        //
        // Probe before opening the stream. `Store::history` builds an ordered
        // push consumer that only terminates once it sees an entry reporting
        // zero pending, so a key that holds no messages at all yields nothing
        // and the stream never ends — the call hangs for the connection's
        // lifetime, and a guest retry loop strands one task per attempt.
        // `entry` returns `Ok(None)` for exactly that case: a delete or purge
        // tombstone still comes back as `Some`, and its history still drains.
        let probed = with_deadline(&conn, "kv.history probe", &key, MAX_HISTORY_DURATION, {
            let store = store.clone();
            let key = key.clone();
            async move { store.entry(&key).await }
        })
        .await;
        match probed {
            Ok(Ok(Some(_))) => {}
            Ok(Ok(None)) => {
                debug!(key = %key, "kv history: key holds nothing, reporting not-found");
                return Ok(Err(types::NatsError::KeyNotFound));
            }
            Ok(Err(e)) => {
                let timed_out = matches!(e.kind(), jetstream::kv::EntryErrorKind::TimedOut);
                return Ok(Err(kv_err("kv history failed", timed_out, e)));
            }
            Err(timeout) => return Ok(Err(timeout)),
        }

        let opened = with_deadline(&conn, "kv.history open", &key, MAX_HISTORY_DURATION, {
            let store = store.clone();
            let key = key.clone();
            async move { store.history(&key).await }
        })
        .await;
        let mut hist = match opened {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => {
                let timed_out = matches!(e.kind(), jetstream::kv::WatchErrorKind::TimedOut);
                return Ok(Err(kv_err("kv history failed", timed_out, e)));
            }
            Err(timeout) => return Ok(Err(timeout)),
        };
        // The probe closes the common case, but history can expire between it
        // and the consumer, so the drain carries its own bound.
        let mut out = Vec::new();
        let budget = conn.request_timeout.unwrap_or(MAX_HISTORY_DURATION);
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            match tokio::time::timeout_at(deadline, hist.next()).await {
                Ok(Some(Ok(e))) => out.push(kv_entry_to_wit(&e)),
                Ok(Some(Err(e))) => {
                    let timed_out = chain_timed_out(&e);
                    return Ok(Err(kv_err("kv history iter failed", timed_out, e)));
                }
                Ok(None) => break,
                Err(_) => {
                    warn!(
                        key = %key,
                        collected = out.len(),
                        "kv history drain did not finish within its deadline; returning a \
                         timeout rather than blocking the guest"
                    );
                    return Ok(Err(types::NatsError::Timeout(format!(
                        // The budget, not the remaining time: the deadline has
                        // already fired here, so a `saturating_duration_since`
                        // against it always renders "within 0ms".
                        "kv history on '{key}' did not complete within {}ms",
                        budget.as_millis()
                    ))));
                }
            }
        }
        if out.is_empty() {
            debug!(key = %key, "kv history drained empty, reporting not-found");
            return Ok(Err(types::NatsError::KeyNotFound));
        }
        debug!(key = %key, entries = out.len(), "kv history returning");
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
