//! # WASI KeyValue Memory Plugin
//!
//! This module implements an in-memory keyvalue plugin for the wasmCloud runtime,
//! providing the `wasi:keyvalue@0.2.0-draft` interfaces for development and testing scenarios.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

const WASI_KEYVALUE_ID: &str = "wasi-keyvalue";
use tokio::sync::RwLock;
use wasmtime::component::Resource;

use crate::{
    engine::{
        ctx::{ActiveCtx, SharedCtx, extract_active_ctx},
        workload::WorkloadItem,
    },
    plugin::HostPlugin,
    wit::{WitInterface, WitWorld},
};

mod bindings {
    wasmtime::component::bindgen!({
        world: "keyvalue",
        imports: { default: async | trappable | tracing },
        with: {
            "wasi:keyvalue/store.bucket": crate::plugin::wasi_keyvalue::in_memory::BucketHandle,
        },
    });
}

use bindings::wasi::keyvalue::store::{Error as StoreError, KeyResponse};

/// In-memory bucket representation
#[derive(Clone, Debug)]
pub struct BucketData {
    pub data: HashMap<String, Vec<u8>>,
}

/// Resource representation for a bucket (key-value store)
pub type BucketHandle = String;

/// Memory-based keyvalue plugin
#[derive(Clone, Default)]
pub struct InMemoryKeyValue {
    /// Storage for all buckets, keyed by workload ID, then bucket name
    storage: Arc<RwLock<HashMap<String, HashMap<String, BucketData>>>>,
}

impl InMemoryKeyValue {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

// Implementation for the store interface
impl<'a> bindings::wasi::keyvalue::store::Host for ActiveCtx<'a> {
    async fn open(
        &mut self,
        identifier: String,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, StoreError>> {
        let Some(plugin) = self.get_plugin::<InMemoryKeyValue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.workload_id.to_string()).or_default();

        // Create bucket if it doesn't exist
        if !workload_storage.contains_key(&identifier) {
            let bucket_data = BucketData {
                data: HashMap::new(),
            };
            workload_storage.insert(identifier.clone(), bucket_data);
        }

        let resource = self.table.push(identifier)?;
        Ok(Ok(resource))
    }
}

// Resource host trait implementations for bucket
impl<'a> bindings::wasi::keyvalue::store::HostBucket for ActiveCtx<'a> {
    async fn get(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<InMemoryKeyValue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage
            .get(&self.workload_id.to_string())
            .unwrap_or(&empty_map);

        match workload_storage.get(bucket_name) {
            Some(bucket_data) => {
                let value = bucket_data.data.get(&key).cloned();
                Ok(Ok(value))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn set(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<InMemoryKeyValue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.workload_id.to_string()).or_default();

        match workload_storage.get_mut(bucket_name) {
            Some(bucket_data) => {
                bucket_data.data.insert(key, value);
                Ok(Ok(()))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn delete(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<InMemoryKeyValue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.workload_id.to_string()).or_default();

        match workload_storage.get_mut(bucket_name) {
            Some(bucket_data) => {
                bucket_data.data.remove(&key);
                Ok(Ok(()))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn exists(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<bool, StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<InMemoryKeyValue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage
            .get(&self.workload_id.to_string())
            .unwrap_or(&empty_map);

        match workload_storage.get(bucket_name) {
            Some(bucket_data) => Ok(Ok(bucket_data.data.contains_key(&key))),
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn list_keys(
        &mut self,
        bucket: Resource<BucketHandle>,
        cursor: Option<u64>,
    ) -> wasmtime::Result<Result<KeyResponse, StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<InMemoryKeyValue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage
            .get(&self.workload_id.to_string())
            .unwrap_or(&empty_map);

        match workload_storage.get(bucket_name) {
            Some(bucket_data) => {
                let mut keys: Vec<String> = bucket_data.data.keys().cloned().collect();
                keys.sort(); // Ensure consistent ordering

                // Simple cursor-based pagination - cursor is the index from previous page
                let start_index = cursor.unwrap_or(0) as usize;

                // Return up to 100 keys per page
                const PAGE_SIZE: usize = 100;
                let end_index = std::cmp::min(start_index + PAGE_SIZE, keys.len());
                let page_keys = keys
                    .get(start_index..end_index)
                    .unwrap_or_default()
                    .to_vec();

                // Set next cursor if there are more keys
                let next_cursor = if end_index < keys.len() {
                    Some(end_index as u64)
                } else {
                    None
                };

                Ok(Ok(KeyResponse {
                    keys: page_keys,
                    cursor: next_cursor,
                }))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn drop(&mut self, rep: Resource<BucketHandle>) -> wasmtime::Result<()> {
        tracing::debug!(
            workload_id = self.workload_id.to_string(),
            resource_id = ?rep,
            "Dropping bucket resource"
        );
        self.table.delete(rep)?;
        Ok(())
    }
}

// Implementation for the atomics interface
impl<'a> bindings::wasi::keyvalue::atomics::Host for ActiveCtx<'a> {
    async fn increment(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        delta: u64,
    ) -> wasmtime::Result<Result<u64, StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<InMemoryKeyValue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.workload_id.to_string()).or_default();

        match workload_storage.get_mut(bucket_name) {
            Some(bucket_data) => {
                // Get current value, treating missing key as 0
                let current_bytes = bucket_data.data.get(&key);
                let current_value = if let Some(bytes) = current_bytes {
                    // Try to parse as u64 from 8-byte array
                    if bytes.len() == 8 {
                        u64::from_le_bytes(bytes.clone().try_into().unwrap_or([0; 8]))
                    } else {
                        // Try to parse as string representation
                        String::from_utf8_lossy(bytes).parse::<u64>().unwrap_or(0)
                    }
                } else {
                    0
                };

                let new_value = current_value.saturating_add(delta);

                // Store as 8-byte little-endian representation
                bucket_data
                    .data
                    .insert(key, new_value.to_le_bytes().to_vec());

                Ok(Ok(new_value))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }
}

// Implementation for the batch interface
impl<'a> bindings::wasi::keyvalue::batch::Host for ActiveCtx<'a> {
    async fn get_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        keys: Vec<String>,
    ) -> wasmtime::Result<Result<Vec<Option<(String, Vec<u8>)>>, StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<InMemoryKeyValue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage
            .get(&self.workload_id.to_string())
            .unwrap_or(&empty_map);

        match workload_storage.get(bucket_name) {
            Some(bucket_data) => {
                let results: Vec<Option<(String, Vec<u8>)>> = keys
                    .into_iter()
                    .map(|key| {
                        bucket_data
                            .data
                            .get(&key)
                            .cloned()
                            .map(|value| (key, value))
                    })
                    .collect();
                Ok(Ok(results))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn set_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<InMemoryKeyValue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.workload_id.to_string()).or_default();

        match workload_storage.get_mut(bucket_name) {
            Some(bucket_data) => {
                for (key, value) in key_values {
                    bucket_data.data.insert(key, value);
                }
                Ok(Ok(()))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn delete_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        keys: Vec<String>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<InMemoryKeyValue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.workload_id.to_string()).or_default();

        match workload_storage.get_mut(bucket_name) {
            Some(bucket_data) => {
                for key in keys {
                    bucket_data.data.remove(&key);
                }
                Ok(Ok(()))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }
}

#[async_trait::async_trait]
impl HostPlugin for InMemoryKeyValue {
    fn id(&self) -> &'static str {
        WASI_KEYVALUE_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasi:keyvalue/store,atomics,batch@0.2.0-draft",
            )]),
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

        let id = component_handle.workload_id();
        tracing::debug!(
            workload_id = id,
            "Successfully added keyvalue interfaces to linker for workload"
        );

        // Initialize storage for this workload
        let mut storage = self.storage.write().await;
        storage.insert(id.to_string(), HashMap::new());

        tracing::debug!("WasiKeyvalue plugin bound to workload '{id}'");

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_id: &str,
        _interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        // Clean up storage for this workload
        let mut storage = self.storage.write().await;
        storage.remove(workload_id);

        tracing::debug!("WasiKeyvalue plugin unbound from workload '{workload_id}'");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_workload_isolation() {
        let kv = InMemoryKeyValue::new();

        // Two workloads writing to the same bucket name should be isolated
        {
            let mut storage = kv.storage.write().await;
            let w1 = storage.entry("workload-1".to_string()).or_default();
            w1.insert(
                "default".to_string(),
                BucketData {
                    data: HashMap::from([("key".to_string(), b"w1-value".to_vec())]),
                },
            );

            let w2 = storage.entry("workload-2".to_string()).or_default();
            w2.insert(
                "default".to_string(),
                BucketData {
                    data: HashMap::from([("key".to_string(), b"w2-value".to_vec())]),
                },
            );
        }

        let storage = kv.storage.read().await;
        assert_eq!(storage["workload-1"]["default"].data["key"], b"w1-value");
        assert_eq!(storage["workload-2"]["default"].data["key"], b"w2-value");
    }

    #[tokio::test]
    async fn test_bucket_set_get_delete() {
        let kv = InMemoryKeyValue::new();
        let workload = "test-workload".to_string();

        // Open/create bucket and set a key
        {
            let mut storage = kv.storage.write().await;
            let ws = storage.entry(workload.clone()).or_default();
            ws.insert(
                "my-bucket".to_string(),
                BucketData {
                    data: HashMap::new(),
                },
            );
            ws.get_mut("my-bucket")
                .unwrap()
                .data
                .insert("counter".to_string(), b"42".to_vec());
        }

        // Get the value back
        {
            let storage = kv.storage.read().await;
            let val = storage[&workload]["my-bucket"].data.get("counter");
            assert_eq!(val, Some(&b"42".to_vec()));
        }

        // Delete the key
        {
            let mut storage = kv.storage.write().await;
            storage
                .get_mut(&workload)
                .unwrap()
                .get_mut("my-bucket")
                .unwrap()
                .data
                .remove("counter");
        }

        // Confirm deleted
        {
            let storage = kv.storage.read().await;
            assert!(!storage[&workload]["my-bucket"].data.contains_key("counter"));
        }
    }

    #[tokio::test]
    async fn test_multiple_buckets_per_workload() {
        let kv = InMemoryKeyValue::new();
        let workload = "wl".to_string();

        {
            let mut storage = kv.storage.write().await;
            let ws = storage.entry(workload.clone()).or_default();
            ws.insert(
                "bucket-a".to_string(),
                BucketData {
                    data: HashMap::from([("k".to_string(), b"a".to_vec())]),
                },
            );
            ws.insert(
                "bucket-b".to_string(),
                BucketData {
                    data: HashMap::from([("k".to_string(), b"b".to_vec())]),
                },
            );
        }

        let storage = kv.storage.read().await;
        assert_eq!(storage[&workload]["bucket-a"].data["k"], b"a");
        assert_eq!(storage[&workload]["bucket-b"].data["k"], b"b");
    }
}
