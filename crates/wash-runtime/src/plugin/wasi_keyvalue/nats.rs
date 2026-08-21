//! # WASI KeyValue NATS Plugin
//!
//! This module implements `wasi:keyvalue@0.2.0-draft` interfaces using
//! NATS JetStream as the backend storage.
//! Atomics are stored in Network Byte Order (big-endian) format.
//!
//! A [`BucketPolicy`] maps the identifier a guest opens onto a physical
//! JetStream bucket and decides whether a missing bucket may be created. The
//! buckets themselves are host-global rather than per workload: two workloads
//! that open the same identifier share one store, which is how they share
//! state deliberately. Set [`BucketPolicy::prefix`] to namespace them apart.
//! The in-memory plugin, by contrast, keys buckets by workload id, so the same
//! guest code is isolated under a bare `wash dev` and shared on NATS.

use std::collections::HashSet;
use std::sync::Arc;

use bytes::{Buf, Bytes};

const PLUGIN_KEYVALUE_ID: &str = "wasi-keyvalue";
use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::wasi_keyvalue::nats_bucket::{BucketPolicy, OpenError, OpenOutcome};
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};
use futures::StreamExt;
use tracing::instrument;
use wasmtime::component::Resource;

const LIST_KEYS_BATCH_SIZE: usize = 1000;

mod bindings {
    wasmtime::component::bindgen!({
        world: "keyvalue",
        imports: { default: async | trappable | tracing },
        with: {
            "wasi:keyvalue/store.bucket": crate::plugin::wasi_keyvalue::nats::BucketHandle,
        },
    });
}

use bindings::wasi::keyvalue::store::{Error as StoreError, KeyResponse};

// Collect one `list-keys` page from `keys`, skipping the first `cursor_skip` keys.
// This will read up to `LIST_KEYS_BATCH_SIZE` + 1 keys from the stream. That way the
// caller will know if there are more keys to read.
async fn collect_key_page<S, E>(keys: S, cursor_skip: u64) -> Result<KeyResponse, String>
where
    S: futures::Stream<Item = Result<String, E>>,
    E: std::fmt::Display,
{
    let mut resp = KeyResponse {
        keys: vec![],
        cursor: None,
    };

    let mut stream = std::pin::pin!(keys);
    let mut skipped: u64 = 0;

    while let Some(key) = stream.next().await {
        // Unwrap the item before deciding whether it is skipped. This prevents
        // an error being counted toward the skip budget, which would silently swallow
        // the error and start the page one key early.
        let key = key.map_err(|e| e.to_string())?;

        if skipped < cursor_skip {
            skipped += 1;
            continue;
        }

        if resp.keys.len() >= LIST_KEYS_BATCH_SIZE {
            resp.cursor = Some(cursor_skip + LIST_KEYS_BATCH_SIZE as u64);
            break;
        }

        resp.keys.push(key);
    }

    Ok(resp)
}

/// Resource representation for a bucket (key-value store)
pub struct BucketHandle {
    kv: async_nats::jetstream::kv::Store,
}

/// NATS JetStream-backed keyvalue plugin
#[derive(Clone)]
pub struct NatsKeyValue {
    client: Arc<async_nats::jetstream::Context>,
    metrics: Arc<WasiKeyvalueMetrics>,
    policy: Arc<BucketPolicy>,
}

struct WasiKeyvalueMetrics {
    operations_total: opentelemetry::metrics::Counter<u64>,
    /// Opens by what the bucket policy did: `existing`, `created`, or
    /// `refused` (the policy declined to create a missing bucket). Separate
    /// from `operations_total` so the policy is measurable without changing
    /// the attributes of the existing series. `created` counts opens that took
    /// the create path, which two racing opens can both do — see
    /// [`OpenOutcome::Created`].
    bucket_opens_total: opentelemetry::metrics::Counter<u64>,
}

impl WasiKeyvalueMetrics {
    fn new(meter: &opentelemetry::metrics::Meter) -> Self {
        let operations_total = meter
            .u64_counter("wasi_keyvalue_operations_total")
            .with_description("Total number of operations performed on the keyvalue store")
            .build();
        let bucket_opens_total = meter
            .u64_counter("wasi_keyvalue_bucket_opens_total")
            .with_description(
                "Bucket opens by outcome: existing, created (this open took the create path), \
                 or refused (the policy declined to create a missing bucket)",
            )
            .build();
        Self {
            operations_total,
            bucket_opens_total,
        }
    }
}

impl NatsKeyValue {
    /// A plugin that creates missing buckets on first open. The development
    /// default: a guest's `open` works against a bare JetStream server with no
    /// configuration. Use [`NatsKeyValue::with_bucket_policy`] to name, prefix,
    /// or lock down the buckets a deployment exposes.
    pub fn new(client: &async_nats::Client) -> Self {
        Self::with_bucket_policy(client, BucketPolicy::create_missing())
    }

    /// A plugin whose opens resolve through `policy`.
    pub fn with_bucket_policy(client: &async_nats::Client, policy: BucketPolicy) -> Self {
        let meter = opentelemetry::global::meter("wasi-keyvalue");
        let metrics = WasiKeyvalueMetrics::new(&meter);
        Self {
            client: async_nats::jetstream::new(client.clone()).into(),
            metrics: Arc::new(metrics),
            policy: Arc::new(policy),
        }
    }

    fn record_operation(&self, operation: &str) {
        let attributes = [opentelemetry::KeyValue::new(
            "operation",
            operation.to_string(),
        )];
        self.metrics.operations_total.add(1, &attributes);
    }

    fn record_bucket_open(&self, outcome: &'static str) {
        let attributes = [opentelemetry::KeyValue::new("outcome", outcome)];
        self.metrics.bucket_opens_total.add(1, &attributes);
    }

    /// Open the JetStream KV bucket `identifier` resolves to under this
    /// plugin's [`BucketPolicy`], creating it only if the policy allows.
    ///
    /// Re-opening an existing, populated bucket returns it with its entries
    /// intact — verified by `tests::test_reopening_bucket_preserves_entries`.
    async fn open_bucket(
        &self,
        identifier: &str,
    ) -> Result<(async_nats::jetstream::kv::Store, OpenOutcome), OpenError> {
        self.policy.open(&self.client, identifier).await
    }
}

// Implementation for the store interface
impl<'a> bindings::wasi::keyvalue::store::Host for ActiveCtx<'a> {
    #[instrument(name = "wasi.keyvalue.open", skip(self))]
    async fn open(
        &mut self,
        identifier: String,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, StoreError>> {
        let plugin = self.try_get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("open");

        let kv = match plugin.open_bucket(&identifier).await {
            Ok((kv, outcome)) => {
                plugin.record_bucket_open(outcome.as_str());
                kv
            }
            Err(OpenError::NoSuchStore) => {
                plugin.record_bucket_open("refused");
                return Ok(Err(StoreError::NoSuchStore));
            }
            Err(OpenError::Other(e)) => return Ok(Err(StoreError::Other(e))),
        };

        let bucket = BucketHandle { kv };

        let resource = self.table.push(bucket)?;
        Ok(Ok(resource))
    }
}

// Resource host trait implementations for bucket
impl<'a> bindings::wasi::keyvalue::store::HostBucket for ActiveCtx<'a> {
    #[instrument(name = "wasi.keyvalue.get", skip(self, bucket))]
    async fn get(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, StoreError>> {
        let plugin = self.try_get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("get");

        let bucket_handle = self.table.get(&bucket)?;

        let entry = match bucket_handle.kv.get(key).await {
            Ok(entry) => entry,
            Err(e) => {
                tracing::error!("JetStream error getting key: {e}");
                return Ok(Err(StoreError::Other(format!("JetStream error: {e}"))));
            }
        };

        match entry {
            Some(e) => Ok(Ok(Some(e.to_vec()))),
            None => Ok(Ok(None)),
        }
    }

    #[instrument(name = "wasi.keyvalue.set", skip(self, bucket, value))]
    async fn set(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let plugin = self.try_get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("set");

        let bucket_handle = self.table.get(&bucket)?;

        match bucket_handle.kv.put(key, value.into()).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("JetStream error setting key: {e}");
                Ok(Err(StoreError::Other(format!("JetStream error: {e}"))))
            }
        }
    }

    #[instrument(name = "wasi.keyvalue.delete", skip(self, bucket))]
    async fn delete(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let plugin = self.try_get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("delete");

        let bucket_handle = self.table.get(&bucket)?;

        match bucket_handle.kv.delete(key).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("JetStream error deleting key: {e}");
                Ok(Err(StoreError::Other(format!("JetStream error: {e}"))))
            }
        }
    }

    #[instrument(name = "wasi.keyvalue.exists", skip(self, bucket))]
    async fn exists(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<bool, StoreError>> {
        let plugin = self.try_get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("exists");

        let bucket_handle = self.table.get(&bucket)?;

        match bucket_handle.kv.get(key).await {
            Ok(Some(_)) => Ok(Ok(true)),
            Ok(None) => Ok(Ok(false)),
            Err(e) => {
                tracing::error!("JetStream error getting key: {e}");
                Ok(Err(StoreError::Other(format!("JetStream error: {e}"))))
            }
        }
    }

    #[instrument(name = "wasi.keyvalue.list_keys", skip(self, bucket))]
    async fn list_keys(
        &mut self,
        bucket: Resource<BucketHandle>,
        cursor: Option<u64>,
    ) -> wasmtime::Result<Result<KeyResponse, StoreError>> {
        let plugin = self.try_get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("list_keys");

        let bucket_handle = self.table.get(&bucket)?;

        let keys_iter = match bucket_handle.kv.keys().await {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("JetStream error getting key: {e}");
                return Ok(Err(StoreError::Other(format!("JetStream error: {e}"))));
            }
        };

        match collect_key_page(keys_iter, cursor.unwrap_or(0)).await {
            Ok(resp) => Ok(Ok(resp)),
            Err(e) => {
                tracing::error!("JetStream error listing keys: {e}");
                Ok(Err(StoreError::Other(format!("JetStream error: {e}"))))
            }
        }
    }

    async fn drop(&mut self, rep: Resource<BucketHandle>) -> wasmtime::Result<()> {
        tracing::debug!(
            workload_id = &*self.workload_id,
            resource_id = ?rep,
            "Dropping bucket resource"
        );
        self.table.delete(rep)?;
        Ok(())
    }
}

// Implementation for the atomics interface
impl<'a> bindings::wasi::keyvalue::atomics::Host for ActiveCtx<'a> {
    #[instrument(name = "wasi.keyvalue.increment", skip(self, bucket))]
    async fn increment(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        delta: u64,
    ) -> wasmtime::Result<Result<u64, StoreError>> {
        let plugin = self.try_get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("increment");

        let bucket_handle = self.table.get(&bucket)?;

        let (entry_revision, entry_value) = match bucket_handle.kv.entry(&key).await {
            Ok(Some(mut e)) => {
                let revision = Some(e.revision);
                // Tolerate a malformed (non-8-byte) value as 0 rather than
                // panicking: `Buf::get_u64` traps the guest if the value has
                // fewer than 8 bytes.
                let value = if e.value.len() >= 8 {
                    e.value.get_u64()
                } else {
                    0
                };
                (revision, value)
            }
            Ok(None) => (None, 0),
            Err(e) => {
                tracing::error!("JetStream error getting key entry: {e}");
                return Ok(Err(StoreError::Other(format!("JetStream error: {e}"))));
            }
        };

        // saturating, so an overflowing increment can't panic-trap the guest.
        let new_value = entry_value.saturating_add(delta);
        let entry_bytes = Bytes::from((new_value).to_be_bytes().to_vec());

        // Here's were CAS happens
        // If we have a revision, we try to update the entry with it
        // If we don't have a revision, we try to create the entry
        match entry_revision {
            Some(rev) => {
                let res = bucket_handle.kv.update(&key, entry_bytes, rev).await;
                match res {
                    Ok(_) => Ok(Ok(new_value)),
                    Err(e) => {
                        tracing::error!("JetStream error updating key: {e}");
                        Ok(Err(StoreError::Other(format!("JetStream error: {e}"))))
                    }
                }
            }
            None => {
                let res = bucket_handle.kv.put(key.clone(), entry_bytes).await;
                match res {
                    Ok(_) => Ok(Ok(new_value)),
                    Err(e) => {
                        tracing::error!("JetStream error putting key: {e}");
                        Ok(Err(StoreError::Other(format!("JetStream error: {e}"))))
                    }
                }
            }
        }
    }
}

// Implementation for the batch interface
impl<'a> bindings::wasi::keyvalue::batch::Host for ActiveCtx<'a> {
    #[instrument(name = "wasi.keyvalue.get_many", skip(self, bucket, keys))]
    #[allow(clippy::type_complexity)]
    async fn get_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        keys: Vec<String>,
    ) -> wasmtime::Result<Result<Vec<Option<(String, Vec<u8>)>>, StoreError>> {
        let plugin = self.try_get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("get_many");

        let bucket_handle = self.table.get(&bucket)?;

        let values = futures::stream::FuturesOrdered::from_iter(keys.iter().map(|key| async {
            match bucket_handle.kv.get(key.clone()).await {
                Ok(Some(entry)) => Ok(Some((key.clone(), entry.to_vec()))),
                Ok(None) => Ok(None),
                Err(e) => {
                    tracing::error!("JetStream error getting key: {e}");
                    Err(StoreError::Other(format!("JetStream error: {e}")))
                }
            }
        }))
        .collect::<Vec<_>>()
        .await;

        // Remove the outer Result, propagate the first error if any
        let mut result = Vec::with_capacity(values.len());
        for entry in values {
            match entry {
                Ok(v) => result.push(v),
                Err(e) => return Ok(Err(e)),
            }
        }

        Ok(Ok(result))
    }

    #[instrument(name = "wasi.keyvalue.set_many", skip(self, bucket, key_values))]
    async fn set_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let plugin = self.try_get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("set_many");

        let bucket_handle = self.table.get(&bucket)?;

        let values = futures::stream::FuturesOrdered::from_iter(key_values.iter().map(
            |(key, value)| async {
                match bucket_handle
                    .kv
                    .put(key.clone(), value.to_vec().into())
                    .await
                {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        tracing::error!("JetStream error putting key: {e}");
                        Err(StoreError::Other(format!("JetStream error: {e}")))
                    }
                }
            },
        ))
        .collect::<Vec<_>>()
        .await;

        // Remove the outer Result, propagate the first error if any
        for entry in values {
            if let Err(e) = entry {
                return Ok(Err(e));
            }
        }

        Ok(Ok(()))
    }

    #[instrument(name = "wasi.keyvalue.delete_many", skip(self, bucket, keys))]
    async fn delete_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        keys: Vec<String>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let plugin = self.try_get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("delete_many");

        let bucket_handle = self.table.get(&bucket)?;

        let values = futures::stream::FuturesOrdered::from_iter(keys.iter().map(|key| async {
            match bucket_handle.kv.delete(key.clone()).await {
                Ok(_) => Ok(()),
                Err(e) => {
                    tracing::error!("JetStream error deleting key: {e}");
                    Err(StoreError::Other(format!("JetStream error: {e}")))
                }
            }
        }))
        .collect::<Vec<_>>()
        .await;

        // Remove the outer Result, propagate the first error if any
        for entry in values {
            if let Err(e) = entry {
                return Ok(Err(e));
            }
        }

        Ok(Ok(()))
    }
}

#[async_trait::async_trait]
impl HostPlugin for NatsKeyValue {
    fn id(&self) -> &'static str {
        PLUGIN_KEYVALUE_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from("wasi:keyvalue/store,atomics,batch")]),
            ..Default::default()
        }
    }

    async fn on_workload_item_bind<'a>(
        &self,
        component_handle: &mut WorkloadItem<'a>,
        interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        // Check if any of the interfaces are wasi:keyvalue related
        if !interfaces.contains("wasi", "keyvalue", &[]) {
            tracing::warn!(
                "WasiKeyvalue plugin requested for non-wasi:keyvalue interface(s): {:?}",
                interfaces
            );
            return Ok(());
        }

        tracing::debug!(
            workload_id = component_handle.id(),
            "Adding keyvalue interfaces to linker for workload"
        );
        let linker = component_handle.linker();

        bindings::wasi::keyvalue::store::add_to_linker::<_, SharedCtx>(linker, extract_active_ctx)?;
        bindings::wasi::keyvalue::atomics::add_to_linker::<_, SharedCtx>(
            linker,
            extract_active_ctx,
        )?;
        bindings::wasi::keyvalue::batch::add_to_linker::<_, SharedCtx>(linker, extract_active_ctx)?;

        let id = component_handle.id();
        tracing::debug!(
            workload_id = id,
            "Successfully added keyvalue interfaces to linker for workload"
        );

        tracing::debug!("WasiKeyvalue plugin bound to component '{id}'");

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: WitInterfaces<'_>,
    ) -> anyhow::Result<()> {
        tracing::debug!("WasiKeyvalue plugin unbound from workload '{workload_id}'");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! Tests for the private `open_bucket` path — the bucket policy's naming
    //! and creation rules — and for the `collect_key_page` pagination helper
    //! `list_keys` delegates to.
    //!
    //! These live here (rather than under `tests/`) because they exercise
    //! private items directly; the repo's `tests/integration_nats_*` files can
    //! only reach the public host API. The bucket tests spin up a real
    //! JetStream via testcontainers (require Docker; marked `#[ignore]`, run
    //! with `cargo test --include-ignored`). The pagination tests run on
    //! synthetic streams and need nothing.

    use super::*;

    /// A stream of `n` distinct `Ok` keys (`key-0`, `key-1`, ...), typed with
    /// a displayable error so it satisfies `collect_key_page`'s bounds.
    fn ok_keys(n: usize) -> impl futures::Stream<Item = Result<String, String>> {
        futures::stream::iter((0..n).map(|i| Ok(format!("key-{i}"))))
    }

    /// A page shorter than the batch size is returned whole with no cursor.
    #[tokio::test]
    async fn collect_key_page_short_page_has_no_cursor() {
        let resp = collect_key_page(ok_keys(3), 0).await.expect("must succeed");
        assert_eq!(resp.keys.len(), 3);
        assert_eq!(resp.cursor, None);
    }

    /// Exactly one full batch: everything fits on one page, so no cursor.
    #[tokio::test]
    async fn collect_key_page_exact_batch_has_no_cursor() {
        let resp = collect_key_page(ok_keys(LIST_KEYS_BATCH_SIZE), 0)
            .await
            .expect("must succeed");
        assert_eq!(resp.keys.len(), LIST_KEYS_BATCH_SIZE);
        assert_eq!(resp.cursor, None);
    }

    /// One key more than a batch: the page is capped at the batch size and
    /// the cursor points at the next page. This is the regression case — the
    /// pre-fix implementation never set the cursor, so callers silently saw
    /// only the first page of large buckets.
    #[tokio::test]
    async fn collect_key_page_overfull_batch_sets_cursor() {
        let resp = collect_key_page(ok_keys(LIST_KEYS_BATCH_SIZE + 1), 0)
            .await
            .expect("must succeed");
        assert_eq!(resp.keys.len(), LIST_KEYS_BATCH_SIZE);
        assert_eq!(resp.cursor, Some(LIST_KEYS_BATCH_SIZE as u64));
    }

    /// Walking the cursor to the end visits every key exactly once.
    #[tokio::test]
    async fn collect_key_page_cursor_walk_covers_all_keys() {
        let total = 2 * LIST_KEYS_BATCH_SIZE + 5;
        let mut seen = Vec::new();
        let mut cursor = 0;
        loop {
            let resp = collect_key_page(ok_keys(total), cursor)
                .await
                .expect("must succeed");
            seen.extend(resp.keys);
            match resp.cursor {
                Some(next) => cursor = next,
                None => break,
            }
        }
        let expected: Vec<String> = (0..total).map(|i| format!("key-{i}")).collect();
        assert_eq!(seen, expected);
    }

    /// A cursor past the end of the listing yields an empty final page.
    #[tokio::test]
    async fn collect_key_page_cursor_past_end_is_empty() {
        let resp = collect_key_page(ok_keys(4), 10)
            .await
            .expect("must succeed");
        assert!(resp.keys.is_empty());
        assert_eq!(resp.cursor, None);
    }

    /// A stream error surfaces as `Err` instead of silently truncating the
    /// page: a partial `Ok` response is indistinguishable from a complete one.
    #[tokio::test]
    async fn collect_key_page_stream_error_is_surfaced() {
        let stream = futures::stream::iter(vec![
            Ok("key-0".to_string()),
            Err("connection reset".to_string()),
            Ok("key-2".to_string()),
        ]);
        let err = collect_key_page(stream, 0)
            .await
            .expect_err("must surface the stream error");
        assert!(err.contains("connection reset"));
    }

    /// An error inside the skipped cursor prefix surfaces as `Err` too — a
    /// count-based skip would swallow it and shift the page one key early.
    #[tokio::test]
    async fn collect_key_page_error_in_skipped_prefix_is_surfaced() {
        let stream = futures::stream::iter(vec![
            Ok("key-0".to_string()),
            Err("connection reset".to_string()),
            Ok("key-2".to_string()),
            Ok("key-3".to_string()),
        ]);
        let err = collect_key_page(stream, 3)
            .await
            .expect_err("must surface the error even though it was skipped over");
        assert!(err.contains("connection reset"));
    }
    use testcontainers::{
        ContainerAsync, GenericImage, ImageExt,
        core::{IntoContainerPort, WaitFor},
        runners::AsyncRunner,
    };

    fn init_tracing() {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init()
            .ok();
    }

    async fn start_nats_jetstream()
    -> anyhow::Result<(ContainerAsync<GenericImage>, async_nats::Client)> {
        let container = GenericImage::new("nats", "2.12.8-alpine")
            .with_exposed_port(4222.tcp())
            .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
            .with_cmd(["-js"])
            .start()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to start NATS container: {e}"))?;

        let port = container
            .get_host_port_ipv4(4222)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get NATS host port: {e}"))?;

        let client = async_nats::connect(format!("nats://127.0.0.1:{port}"))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to connect to NATS: {e}"))?;

        Ok((container, client))
    }

    /// Re-opening an existing, populated bucket returns the same store with its
    /// entries intact: the create-if-missing path must not reset or wipe a
    /// bucket that is already there.
    #[tokio::test]
    #[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
    async fn test_reopening_bucket_preserves_entries() -> anyhow::Result<()> {
        init_tracing();

        let (_container, client) = start_nats_jetstream().await?;
        let kv = NatsKeyValue::new(&client);
        let bucket = format!("kv-{}", uuid::Uuid::new_v4());

        let (created, outcome) = kv
            .open_bucket(&bucket)
            .await
            .map_err(|e| anyhow::anyhow!("first open failed: {e}"))?;
        assert_eq!(outcome, OpenOutcome::Created);
        created
            .put("greeting", bytes::Bytes::from_static(b"hello"))
            .await
            .map_err(|e| anyhow::anyhow!("put failed: {e}"))?;

        let (reopened, outcome) = kv
            .open_bucket(&bucket)
            .await
            .map_err(|e| anyhow::anyhow!("re-open failed: {e}"))?;
        assert_eq!(
            outcome,
            OpenOutcome::Existing,
            "the second open must find the bucket, not recreate it"
        );
        let value = reopened
            .get("greeting")
            .await
            .map_err(|e| anyhow::anyhow!("get failed: {e}"))?;

        assert_eq!(
            value.as_deref(),
            Some(b"hello".as_slice()),
            "entry was lost when re-opening the bucket"
        );

        Ok(())
    }

    /// Under `create = never` a guest cannot bring a bucket into existence: an
    /// unknown identifier is `no-such-store`, and only a bucket the operator
    /// created is reachable.
    #[tokio::test]
    #[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
    async fn test_create_never_rejects_unknown_bucket() -> anyhow::Result<()> {
        init_tracing();

        let (_container, client) = start_nats_jetstream().await?;
        let kv = NatsKeyValue::with_bucket_policy(&client, BucketPolicy::default());
        let bucket = format!("kv-{}", uuid::Uuid::new_v4());

        let err = kv
            .open_bucket(&bucket)
            .await
            .expect_err("opening an absent bucket must fail under create = never");
        assert!(
            matches!(err, OpenError::NoSuchStore),
            "expected no-such-store, got {err}"
        );

        // The same identifier opens once the bucket exists.
        async_nats::jetstream::new(client.clone())
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket: bucket.clone(),
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow::anyhow!("operator create failed: {e}"))?;
        kv.open_bucket(&bucket)
            .await
            .map_err(|e| anyhow::anyhow!("open of an existing bucket failed: {e}"))?;

        Ok(())
    }

    /// The prefix is applied to the physical bucket, so the same guest
    /// identifier lands in different stores for differently-prefixed policies.
    #[tokio::test]
    #[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
    async fn test_prefix_separates_buckets() -> anyhow::Result<()> {
        init_tracing();

        let (_container, client) = start_nats_jetstream().await?;
        let identifier = format!("kv-{}", uuid::Uuid::new_v4());

        let mut policy = BucketPolicy::create_missing();
        policy.prefix = "a-".to_string();
        let a = NatsKeyValue::with_bucket_policy(&client, policy.clone());
        policy.prefix = "b-".to_string();
        let b = NatsKeyValue::with_bucket_policy(&client, policy);

        let (a_bucket, _) = a
            .open_bucket(&identifier)
            .await
            .map_err(|e| anyhow::anyhow!("open a failed: {e}"))?;
        a_bucket
            .put("greeting", bytes::Bytes::from_static(b"hello"))
            .await
            .map_err(|e| anyhow::anyhow!("put failed: {e}"))?;

        let (b_bucket, _) = b
            .open_bucket(&identifier)
            .await
            .map_err(|e| anyhow::anyhow!("open b failed: {e}"))?;
        let value = b_bucket
            .get("greeting")
            .await
            .map_err(|e| anyhow::anyhow!("get failed: {e}"))?;

        assert!(
            value.is_none(),
            "the prefixed policies must not share a bucket"
        );

        Ok(())
    }

    /// Creation settings reach the bucket JetStream creates.
    #[tokio::test]
    #[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
    async fn test_creation_settings_apply_to_new_bucket() -> anyhow::Result<()> {
        init_tracing();

        let (_container, client) = start_nats_jetstream().await?;
        let identifier = format!("kv-{}", uuid::Uuid::new_v4());

        let mut policy = BucketPolicy::create_missing();
        policy.history = Some(5);
        policy.max_age = Some(std::time::Duration::from_secs(3600));
        let kv = NatsKeyValue::with_bucket_policy(&client, policy);

        kv.open_bucket(&identifier)
            .await
            .map_err(|e| anyhow::anyhow!("open failed: {e}"))?;

        let status = async_nats::jetstream::new(client.clone())
            .get_key_value(&identifier)
            .await
            .map_err(|e| anyhow::anyhow!("get_key_value failed: {e}"))?
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("status failed: {e}"))?;

        assert_eq!(status.history(), 5);
        assert_eq!(status.max_age(), std::time::Duration::from_secs(3600));

        Ok(())
    }
}
