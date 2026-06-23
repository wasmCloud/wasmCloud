#![deny(clippy::all)]
//! # WASI KeyValue Filesystem Plugin
//!
//! Implements `wasi:keyvalue@0.2.0-draft` over the local filesystem. The actual
//! storage lives in the shared [`FsKvStore`](super::fs_store::FsKvStore) — this
//! module is the host-binding adapter (the unnamed/default `wasi:keyvalue`
//! instance); the multiplexed `FilesystemBackend` is the other adapter.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

const PLUGIN_KEYVALUE_ID: &str = "wasi-keyvalue";
use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::wasi_keyvalue::fs_store::{FsKvError, FsKvStore};
use crate::plugin::{HostPlugin, WitInterfaces};
use crate::wit::{WitInterface, WitWorld};
use tracing::instrument;
use wasmtime::component::Resource;

const LIST_KEYS_BATCH_SIZE: usize = 1000;

mod bindings {
    wasmtime::component::bindgen!({
        world: "keyvalue",
        imports: { default: async | trappable | tracing },
        with: {
            "wasi:keyvalue/store.bucket": crate::plugin::wasi_keyvalue::filesystem::BucketHandle,
        },
    });
}

use bindings::wasi::keyvalue::store::{Error as StoreError, KeyResponse};

/// Map a shared-store error to the WIT `store::Error`.
fn to_store_error(e: FsKvError) -> StoreError {
    match e {
        FsKvError::InvalidIdentifier => {
            StoreError::Other("invalid keyvalue identifier".to_string())
        }
        FsKvError::Io(e) => StoreError::Other(format!("Filesystem error: {e}")),
    }
}

/// Resource representation for a bucket: its identifier (a subdirectory of the
/// store root, resolved per-op by [`FsKvStore`]).
pub struct BucketHandle {
    id: String,
}

/// Filesystem-based keyvalue plugin.
#[derive(Clone)]
pub struct FilesystemKeyValue {
    store: FsKvStore,
    metrics: Arc<WasiKeyvalueMetrics>,
}

struct WasiKeyvalueMetrics {
    operations_total: opentelemetry::metrics::Counter<u64>,
}

impl WasiKeyvalueMetrics {
    fn new(meter: &opentelemetry::metrics::Meter) -> Self {
        let operations_total = meter
            .u64_counter("wasi_keyvalue_operations_total")
            .with_description("Total number of operations performed on the keyvalue store")
            .build();
        Self { operations_total }
    }
}

impl FilesystemKeyValue {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let meter = opentelemetry::global::meter("wasi-keyvalue");
        let metrics = WasiKeyvalueMetrics::new(&meter);
        Self {
            store: FsKvStore::new(root),
            metrics: Arc::new(metrics),
        }
    }

    fn record_operation(&self, operation: &str) {
        let attributes = [opentelemetry::KeyValue::new(
            "operation",
            operation.to_string(),
        )];
        self.metrics.operations_total.add(1, &attributes);
    }
}

// Implementation for the store interface
impl<'a> bindings::wasi::keyvalue::store::Host for ActiveCtx<'a> {
    #[instrument(name = "wasi.keyvalue.open", skip(self))]
    async fn open(
        &mut self,
        identifier: String,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, StoreError>> {
        let plugin = self.try_get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID)?;
        plugin.record_operation("open");

        if let Err(e) = plugin.store.create_bucket(&identifier).await {
            return Ok(Err(to_store_error(e)));
        }

        let resource = self.table.push(BucketHandle { id: identifier })?;
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
        let plugin = self.try_get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID)?;
        plugin.record_operation("get");

        let id = self.table.get(&bucket)?.id.clone();
        Ok(plugin.store.get(&id, &key).await.map_err(to_store_error))
    }

    #[instrument(name = "wasi.keyvalue.set", skip(self, bucket, value))]
    async fn set(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let plugin = self.try_get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID)?;
        plugin.record_operation("set");

        let id = self.table.get(&bucket)?.id.clone();
        Ok(plugin
            .store
            .set(&id, &key, &value)
            .await
            .map_err(to_store_error))
    }

    #[instrument(name = "wasi.keyvalue.delete", skip(self, bucket))]
    async fn delete(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let plugin = self.try_get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID)?;
        plugin.record_operation("delete");

        let id = self.table.get(&bucket)?.id.clone();
        Ok(plugin.store.delete(&id, &key).await.map_err(to_store_error))
    }

    #[instrument(name = "wasi.keyvalue.exists", skip(self, bucket))]
    async fn exists(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<bool, StoreError>> {
        let plugin = self.try_get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID)?;
        plugin.record_operation("exists");

        let id = self.table.get(&bucket)?.id.clone();
        Ok(plugin.store.exists(&id, &key).await.map_err(to_store_error))
    }

    #[instrument(name = "wasi.keyvalue.list_keys", skip(self, bucket))]
    async fn list_keys(
        &mut self,
        bucket: Resource<BucketHandle>,
        cursor: Option<u64>,
    ) -> wasmtime::Result<Result<KeyResponse, StoreError>> {
        let plugin = self.try_get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID)?;
        plugin.record_operation("list_keys");

        let id = self.table.get(&bucket)?.id.clone();
        match plugin
            .store
            .list_keys(&id, cursor, LIST_KEYS_BATCH_SIZE)
            .await
        {
            Ok((keys, cursor)) => Ok(Ok(KeyResponse { keys, cursor })),
            Err(e) => Ok(Err(to_store_error(e))),
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
        let plugin = self.try_get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID)?;
        plugin.record_operation("increment");

        let id = self.table.get(&bucket)?.id.clone();
        Ok(plugin
            .store
            .increment(&id, &key, delta)
            .await
            .map_err(to_store_error))
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
        let plugin = self.try_get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID)?;
        plugin.record_operation("get_many");

        let id = self.table.get(&bucket)?.id.clone();
        let mut result = Vec::with_capacity(keys.len());
        for key in keys {
            match plugin.store.get(&id, &key).await {
                Ok(value) => result.push(value.map(|v| (key, v))),
                Err(e) => return Ok(Err(to_store_error(e))),
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
        let plugin = self.try_get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID)?;
        plugin.record_operation("set_many");

        let id = self.table.get(&bucket)?.id.clone();
        for (key, value) in key_values {
            if let Err(e) = plugin.store.set(&id, &key, &value).await {
                return Ok(Err(to_store_error(e)));
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
        let plugin = self.try_get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID)?;
        plugin.record_operation("delete_many");

        let id = self.table.get(&bucket)?.id.clone();
        for key in keys {
            if let Err(e) = plugin.store.delete(&id, &key).await {
                return Ok(Err(to_store_error(e)));
            }
        }
        Ok(Ok(()))
    }
}

#[async_trait::async_trait]
impl HostPlugin for FilesystemKeyValue {
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
