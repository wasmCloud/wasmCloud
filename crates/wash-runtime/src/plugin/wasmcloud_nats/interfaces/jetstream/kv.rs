//! `wasmcloud:nats/kv@0.1.0` — the JetStream KV store, and the `bucket`
//! resource an `open` hands back.

use std::time::Duration;

use async_nats::jetstream;
use futures::StreamExt as _;
use tracing::{debug, warn};
use wasmtime::component::{Accessor, Resource};

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
