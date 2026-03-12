//! # WASI KeyValue Memory Plugin
//!
//! This module implements `wasi:keyvalue@0.2.0-draft` interfaces using
//! NATS JetStream as the backend storage.
//! Atomics are stored in Network Byte Order (big-endian) format.

use std::collections::HashSet;
use std::sync::Arc;

use bytes::{Buf, Bytes};

const PLUGIN_KEYVALUE_ID: &str = "wasi-keyvalue";
use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::HostPlugin;
use crate::wit::{WitInterface, WitWorld};
use futures::StreamExt;
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

/// Resource representation for a bucket (key-value store)
pub struct BucketHandle {
    kv: async_nats::jetstream::kv::Store,
}

/// Memory-based keyvalue plugin
#[derive(Clone)]
pub struct NatsKeyValue {
    client: Arc<async_nats::jetstream::Context>,
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

impl NatsKeyValue {
    pub fn new(client: &async_nats::Client) -> Self {
        let meter = opentelemetry::global::meter("wasi-keyvalue");
        let metrics = WasiKeyvalueMetrics::new(&meter);
        Self {
            client: async_nats::jetstream::new(client.clone()).into(),
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
    async fn open(
        &mut self,
        identifier: String,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, StoreError>> {
        let Some(plugin) = self.get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("open");

        let kv = match plugin.client.get_key_value(&identifier).await {
            Ok(kv) => {
                tracing::debug!("Opened existing bucket in JetStream");
                kv
            }
            Err(e) => {
                tracing::error!("Bucket not found in JetStream({identifier}): {e}");
                return Ok(Err(StoreError::Other(
                    "failed to get keyvalue from JetStream".to_string(),
                )));
            }
        };

        let bucket = BucketHandle { kv };

        let resource = self.table.push(bucket)?;
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
        let Some(plugin) = self.get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("get");

        let bucket_handle = self.table.get(&bucket)?;

        let entry = match bucket_handle.kv.get(key).await {
            Ok(entry) => entry,
            Err(e) => {
                tracing::error!("JetStream error getting key: {}", e);
                return Ok(Err(StoreError::Other(format!("JetStream error: {}", e))));
            }
        };

        match entry {
            Some(e) => Ok(Ok(Some(e.to_vec()))),
            None => Ok(Ok(None)),
        }
    }

    async fn set(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let Some(plugin) = self.get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("get");

        let bucket_handle = self.table.get(&bucket)?;

        match bucket_handle.kv.put(key, value.into()).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("JetStream error setting key: {}", e);
                Ok(Err(StoreError::Other(format!("JetStream error: {}", e))))
            }
        }
    }

    async fn delete(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let Some(plugin) = self.get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("delete");

        let bucket_handle = self.table.get(&bucket)?;

        match bucket_handle.kv.delete(key).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("JetStream error deleting key: {}", e);
                Ok(Err(StoreError::Other(format!("JetStream error: {}", e))))
            }
        }
    }

    async fn exists(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<bool, StoreError>> {
        let Some(plugin) = self.get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("exists");

        let bucket_handle = self.table.get(&bucket)?;

        match bucket_handle.kv.get(key).await {
            Ok(Some(_)) => Ok(Ok(true)),
            Ok(None) => Ok(Ok(false)),
            Err(e) => {
                tracing::error!("JetStream error getting key: {}", e);
                Ok(Err(StoreError::Other(format!("JetStream error: {}", e))))
            }
        }
    }

    async fn list_keys(
        &mut self,
        bucket: Resource<BucketHandle>,
        cursor: Option<u64>,
    ) -> wasmtime::Result<Result<KeyResponse, StoreError>> {
        let Some(plugin) = self.get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("list_keys");

        let bucket_handle = self.table.get(&bucket)?;

        let keys_iter = match bucket_handle.kv.keys().await {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("JetStream error getting key: {}", e);
                return Ok(Err(StoreError::Other(format!("JetStream error: {}", e))));
            }
        };

        let mut resp = KeyResponse {
            keys: vec![],
            cursor: None,
        };

        let cursor_skip = cursor.unwrap_or(0) as usize;

        let mut stream = keys_iter
            .skip(cursor_skip)
            .take(LIST_KEYS_BATCH_SIZE + 1)
            .boxed();

        while let Some(Ok(key)) = stream.next().await {
            if resp.keys.len() > LIST_KEYS_BATCH_SIZE {
                resp.cursor = Some(cursor_skip as u64 + LIST_KEYS_BATCH_SIZE as u64);
                break;
            }

            resp.keys.push(key);
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
    async fn increment(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        delta: u64,
    ) -> wasmtime::Result<Result<u64, StoreError>> {
        let Some(plugin) = self.get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("increment");

        let bucket_handle = self.table.get(&bucket)?;

        let (entry_revision, entry_value) = match bucket_handle.kv.entry(&key).await {
            Ok(Some(mut e)) => {
                let revision = Some(e.revision);
                let value = e.value.get_u64();
                (revision, value)
            }
            Ok(None) => (None, 0),
            Err(e) => {
                tracing::error!("JetStream error getting key entry: {}", e);
                return Ok(Err(StoreError::Other(format!("JetStream error: {}", e))));
            }
        };

        let new_value = entry_value + delta;
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
                        tracing::error!("JetStream error updating key: {}", e);
                        Ok(Err(StoreError::Other(format!("JetStream error: {}", e))))
                    }
                }
            }
            None => {
                let res = bucket_handle.kv.put(key.clone(), entry_bytes).await;
                match res {
                    Ok(_) => Ok(Ok(new_value)),
                    Err(e) => {
                        tracing::error!("JetStream error putting key: {}", e);
                        Ok(Err(StoreError::Other(format!("JetStream error: {}", e))))
                    }
                }
            }
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
        let Some(plugin) = self.get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("get_many");

        let bucket_handle = self.table.get(&bucket)?;

        let values = futures::stream::FuturesOrdered::from_iter(keys.iter().map(|key| async {
            match bucket_handle.kv.get(key.clone()).await {
                Ok(Some(entry)) => Ok(Some((key.clone(), entry.to_vec()))),
                Ok(None) => Ok(None),
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

    async fn set_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let Some(plugin) = self.get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
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

    async fn delete_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        keys: Vec<String>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let Some(plugin) = self.get_plugin::<NatsKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("delete_many");

        let bucket_handle = self.table.get(&bucket)?;

        let values = futures::stream::FuturesOrdered::from_iter(keys.iter().map(|key| async {
            match bucket_handle.kv.delete(key.clone()).await {
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
