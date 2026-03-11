#![deny(clippy::all)]
//! # WASI KeyValue Memory Plugin
//!
//! This module implements `wasi:keyvalue@0.2.0-draft` interfaces using
//! Filesystem  as the backend storage.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const PLUGIN_KEYVALUE_ID: &str = "wasi-keyvalue";
use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::{HostPlugin, lock_root};
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
            "wasi:keyvalue/store.bucket": crate::plugin::wasi_keyvalue::filesystem::BucketHandle,
        },
    });
}

use bindings::wasi::keyvalue::store::{Error as StoreError, KeyResponse};

/// Resource representation for a bucket (key-value store)
pub struct BucketHandle {
    root: PathBuf,
}

/// Filesystem-based keyvalue plugin
#[derive(Clone)]
pub struct FilesystemKeyValue {
    root: PathBuf,
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
            root: root.as_ref().to_path_buf(),
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
    #[instrument(skip(self))]
    async fn open(
        &mut self,
        identifier: String,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, StoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("open");

        let Ok(path) = lock_root(&plugin.root, &identifier) else {
            return Ok(Err(StoreError::Other(
                "invalid bucket identifier".to_string(),
            )));
        };

        if std::fs::create_dir_all(&path).is_err() {
            return Ok(Err(StoreError::Other(
                "failed to create bucket directory".to_string(),
            )));
        }

        let bucket = BucketHandle { root: path };

        let resource = self.table.push(bucket)?;
        Ok(Ok(resource))
    }
}

// Resource host trait implementations for bucket
impl<'a> bindings::wasi::keyvalue::store::HostBucket for ActiveCtx<'a> {
    #[instrument(skip(self, bucket))]
    async fn get(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, StoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("get");

        let bucket_handle = self.table.get(&bucket)?;
        let Ok(path) = lock_root(&bucket_handle.root, &key) else {
            return Ok(Err(StoreError::Other("invalid key identifier".to_string())));
        };

        let entry = match tokio::fs::read(path).await {
            Ok(entry) => Some(entry.to_vec()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Ok(Err(StoreError::Other(format!("Filesystem error: {}", e))));
            }
        };

        Ok(Ok(entry))
    }

    #[instrument(skip(self, bucket, value))]
    async fn set(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("set");

        let bucket_handle = self.table.get(&bucket)?;

        let Ok(path) = lock_root(&bucket_handle.root, &key) else {
            return Ok(Err(StoreError::Other("invalid key identifier".to_string())));
        };

        match tokio::fs::write(path, value).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("Filesystem error setting key: {}", e);
                Ok(Err(StoreError::Other(format!("Filesystem error: {}", e))))
            }
        }
    }

    #[instrument(skip(self, bucket))]
    async fn delete(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("delete");

        let bucket_handle = self.table.get(&bucket)?;
        let Ok(path) = lock_root(&bucket_handle.root, &key) else {
            return Ok(Err(StoreError::Other("invalid key identifier".to_string())));
        };

        match tokio::fs::remove_file(path).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("Filesystem error deleting key: {}", e);
                Ok(Err(StoreError::Other(format!("Filesystem error: {}", e))))
            }
        }
    }

    #[instrument(skip(self, bucket))]
    async fn exists(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<bool, StoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("exists");

        let bucket_handle = self.table.get(&bucket)?;

        let Ok(path) = lock_root(&bucket_handle.root, &key) else {
            return Ok(Err(StoreError::Other("invalid key identifier".to_string())));
        };

        // directories are not valid keys
        if path.is_dir() {
            return Ok(Ok(false));
        }

        Ok(Ok(path.exists()))
    }

    #[instrument(skip(self, bucket))]
    async fn list_keys(
        &mut self,
        bucket: Resource<BucketHandle>,
        cursor: Option<u64>,
    ) -> wasmtime::Result<Result<KeyResponse, StoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("list_keys");

        let bucket_handle = self.table.get(&bucket)?;

        let mut keys_iter = match tokio::fs::read_dir(&bucket_handle.root).await {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("Filesystem error getting key: {}", e);
                return Ok(Err(StoreError::Other(format!("Filesystem error: {}", e))));
            }
        };

        let mut resp = KeyResponse {
            keys: vec![],
            cursor: None,
        };

        let cursor_skip = cursor.unwrap_or(0) as usize;
        let mut cursor_done = cursor_skip;

        while let Ok(Some(key)) = keys_iter.next_entry().await {
            // skip keys until we reach the cursor
            if cursor_done != 0 {
                cursor_done -= 1;
                continue;
            }

            // if we have reached the batch size, set the cursor and break
            if resp.keys.len() >= LIST_KEYS_BATCH_SIZE {
                resp.cursor = Some(cursor_skip as u64 + LIST_KEYS_BATCH_SIZE as u64);
                break;
            }

            resp.keys
                .push(key.file_name().to_string_lossy().to_string());
        }

        Ok(Ok(resp))
    }

    async fn drop(&mut self, rep: Resource<BucketHandle>) -> wasmtime::Result<()> {
        tracing::debug!(
            workload_id = self.id,
            resource_id = ?rep,
            "Dropping bucket resource"
        );
        self.table.delete(rep)?;
        Ok(())
    }
}

// Implementation for the atomics interface
impl<'a> bindings::wasi::keyvalue::atomics::Host for ActiveCtx<'a> {
    #[instrument(skip(self, bucket))]
    async fn increment(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        delta: u64,
    ) -> wasmtime::Result<Result<u64, StoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("increment");

        let bucket_handle = self.table.get(&bucket)?;
        let Ok(path) = lock_root(&bucket_handle.root, &key) else {
            return Ok(Err(StoreError::Other("invalid key identifier".to_string())));
        };

        let current_value = match tokio::fs::read_to_string(&path).await {
            Ok(entry) => entry.trim().parse::<u64>().unwrap_or(0),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => {
                tracing::error!("Filesystem error getting key entry: {}", e);
                return Ok(Err(StoreError::Other(format!("Filesystem error: {}", e))));
            }
        };

        let new_value = current_value + delta;

        match tokio::fs::write(&path, new_value.to_string()).await {
            Ok(_) => Ok(Ok(new_value)),
            Err(e) => {
                tracing::error!("Filesystem error putting key: {}", e);
                Ok(Err(StoreError::Other(format!("Filesystem error: {}", e))))
            }
        }
    }
}

// Implementation for the batch interface
impl<'a> bindings::wasi::keyvalue::batch::Host for ActiveCtx<'a> {
    #[instrument(skip(self, bucket, keys))]
    #[allow(clippy::type_complexity)]
    async fn get_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        keys: Vec<String>,
    ) -> wasmtime::Result<Result<Vec<Option<(String, Vec<u8>)>>, StoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("get_many");

        let bucket_handle = self.table.get(&bucket)?;

        let values = futures::stream::FuturesOrdered::from_iter(keys.iter().map(|key| async {
            let Ok(path) = lock_root(&bucket_handle.root, key) else {
                return Err(StoreError::Other("invalid key identifier".to_string()));
            };
            match tokio::fs::read(path).await {
                Ok(entry) => Ok(Some((key.clone(), entry.to_vec()))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => {
                    tracing::error!("JetStream error getting key: {}", e);
                    Err(StoreError::Other(format!("JetStream error: {}", e)))
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

    #[instrument(skip(self, bucket, key_values))]
    async fn set_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("set_many");

        let bucket_handle = self.table.get(&bucket)?;

        let values = futures::stream::FuturesOrdered::from_iter(key_values.iter().map(
            |(key, value)| async {
                let Ok(path) = lock_root(&bucket_handle.root, key) else {
                    return Err(StoreError::Other("invalid key identifier".to_string()));
                };
                match tokio::fs::write(path, value.to_vec()).await {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        tracing::error!("JetStream error putting key: {}", e);
                        Err(StoreError::Other(format!("JetStream error: {}", e)))
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

    #[instrument(skip(self, bucket, keys))]
    async fn delete_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        keys: Vec<String>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let Some(plugin) = self.get_plugin::<FilesystemKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("delete_many");

        let bucket_handle = self.table.get(&bucket)?;

        let values = futures::stream::FuturesOrdered::from_iter(keys.iter().map(|key| async {
            let Ok(path) = lock_root(&bucket_handle.root, key) else {
                return Err(StoreError::Other("invalid key identifier".to_string()));
            };
            match tokio::fs::remove_file(path).await {
                Ok(_) => Ok(()),
                Err(e) => {
                    tracing::error!("JetStream error deleting key: {}", e);
                    Err(StoreError::Other(format!("JetStream error: {}", e)))
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
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        // Check if any of the interfaces are wasi:keyvalue related
        let has_keyvalue = interfaces
            .iter()
            .any(|i| i.namespace == "wasi" && i.package == "keyvalue");

        if !has_keyvalue {
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
        _interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        tracing::debug!("WasiKeyvalue plugin unbound from workload '{workload_id}'");

        Ok(())
    }
}
