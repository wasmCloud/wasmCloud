//! # WASI KeyValue Redis Plugin
//!
//! This module implements `wasi:keyvalue@0.2.0-draft` interfaces using
//! Redis as the backend storage.
//! The `open` identifier is used as a key prefix (`{identifier}:{key}`) to
//! namespace keys within a single Redis database.

use std::collections::HashSet;
use std::sync::Arc;

const PLUGIN_KEYVALUE_ID: &str = "wasi-keyvalue";
use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
use crate::plugin::HostPlugin;
use crate::wit::{WitInterface, WitWorld};
use futures::StreamExt;
use redis::AsyncCommands;
use wasmtime::component::Resource;

const LIST_KEYS_BATCH_SIZE: usize = 1000;

mod bindings {
    wasmtime::component::bindgen!({
        world: "keyvalue",
        imports: { default: async | trappable | tracing },
        with: {
            "wasi:keyvalue/store.bucket": crate::plugin::wasi_keyvalue::redis::BucketHandle,
        },
    });
}

use bindings::wasi::keyvalue::store::{Error as StoreError, KeyResponse};

/// Resource representation for a bucket (key-value store namespace)
pub struct BucketHandle {
    conn: redis::aio::MultiplexedConnection,
    prefix: String,
}

impl BucketHandle {
    fn prefixed_key(&self, key: &str) -> String {
        format!("{}:{}", self.prefix, key)
    }
}

/// Redis-based keyvalue plugin
#[derive(Clone)]
pub struct RedisKeyValue {
    client: redis::Client,
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

impl RedisKeyValue {
    pub fn new(client: redis::Client) -> Self {
        let meter = opentelemetry::global::meter("wasi-keyvalue");
        let metrics = WasiKeyvalueMetrics::new(&meter);
        Self {
            client,
            metrics: Arc::new(metrics),
        }
    }

    pub fn from_url(url: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(url)?;
        Ok(Self::new(client))
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
    ) -> anyhow::Result<Result<Resource<BucketHandle>, StoreError>> {
        let Some(plugin) = self.get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("open");

        let conn = match plugin.client.get_multiplexed_async_connection().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!("Failed to connect to Redis: {e}");
                return Ok(Err(StoreError::Other(
                    "failed to connect to Redis".to_string(),
                )));
            }
        };

        let bucket = BucketHandle {
            conn,
            prefix: identifier,
        };

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
    ) -> anyhow::Result<Result<Option<Vec<u8>>, StoreError>> {
        let Some(plugin) = self.get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("get");

        let bucket_handle = self.table.get(&bucket)?;
        let mut conn = bucket_handle.conn.clone();
        let redis_key = bucket_handle.prefixed_key(&key);

        match conn.get::<_, Option<Vec<u8>>>(redis_key).await {
            Ok(value) => Ok(Ok(value)),
            Err(e) => {
                tracing::error!("Redis error getting key: {}", e);
                Ok(Err(StoreError::Other(format!("Redis error: {}", e))))
            }
        }
    }

    async fn set(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> anyhow::Result<Result<(), StoreError>> {
        let Some(plugin) = self.get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("set");

        let bucket_handle = self.table.get(&bucket)?;
        let mut conn = bucket_handle.conn.clone();
        let redis_key = bucket_handle.prefixed_key(&key);

        match conn.set::<_, _, ()>(redis_key, value).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("Redis error setting key: {}", e);
                Ok(Err(StoreError::Other(format!("Redis error: {}", e))))
            }
        }
    }

    async fn delete(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> anyhow::Result<Result<(), StoreError>> {
        let Some(plugin) = self.get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("delete");

        let bucket_handle = self.table.get(&bucket)?;
        let mut conn = bucket_handle.conn.clone();
        let redis_key = bucket_handle.prefixed_key(&key);

        match conn.del::<_, ()>(redis_key).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("Redis error deleting key: {}", e);
                Ok(Err(StoreError::Other(format!("Redis error: {}", e))))
            }
        }
    }

    async fn exists(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> anyhow::Result<Result<bool, StoreError>> {
        let Some(plugin) = self.get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("exists");

        let bucket_handle = self.table.get(&bucket)?;
        let mut conn = bucket_handle.conn.clone();
        let redis_key = bucket_handle.prefixed_key(&key);

        match conn.exists::<_, bool>(redis_key).await {
            Ok(exists) => Ok(Ok(exists)),
            Err(e) => {
                tracing::error!("Redis error checking key existence: {}", e);
                Ok(Err(StoreError::Other(format!("Redis error: {}", e))))
            }
        }
    }

    async fn list_keys(
        &mut self,
        bucket: Resource<BucketHandle>,
        cursor: Option<u64>,
    ) -> anyhow::Result<Result<KeyResponse, StoreError>> {
        let Some(plugin) = self.get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("list_keys");

        let bucket_handle = self.table.get(&bucket)?;
        let mut conn = bucket_handle.conn.clone();
        let pattern = bucket_handle.prefixed_key("*");
        let redis_cursor = cursor.unwrap_or(0);

        let (next_cursor, raw_keys): (u64, Vec<String>) = match redis::cmd("SCAN")
            .arg(redis_cursor)
            .arg("MATCH")
            .arg(&pattern)
            .arg("COUNT")
            .arg(LIST_KEYS_BATCH_SIZE)
            .query_async(&mut conn)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                tracing::error!("Redis error listing keys: {}", e);
                return Ok(Err(StoreError::Other(format!("Redis error: {}", e))));
            }
        };

        let key_prefix = pattern.strip_suffix('*').unwrap_or("");
        let keys = raw_keys
            .into_iter()
            .filter_map(|k| k.strip_prefix(key_prefix).map(str::to_string))
            .collect();

        // Redis returns cursor 0 when a full iteration has completed
        let out_cursor = if next_cursor == 0 {
            None
        } else {
            Some(next_cursor)
        };

        Ok(Ok(KeyResponse {
            keys,
            cursor: out_cursor,
        }))
    }

    async fn drop(&mut self, rep: Resource<BucketHandle>) -> anyhow::Result<()> {
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
// Atomics use Redis' native INCRBY for atomic increment, storing values as
// Redis integer strings rather than big-endian bytes.
impl<'a> bindings::wasi::keyvalue::atomics::Host for ActiveCtx<'a> {
    async fn increment(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        delta: u64,
    ) -> anyhow::Result<Result<u64, StoreError>> {
        let Some(plugin) = self.get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("increment");

        let bucket_handle = self.table.get(&bucket)?;
        let mut conn = bucket_handle.conn.clone();
        let redis_key = bucket_handle.prefixed_key(&key);

        let delta_i64 = i64::try_from(delta)
            .map_err(|_| anyhow::anyhow!("delta value {} exceeds i64::MAX", delta))?;
        match conn.incr::<_, _, i64>(redis_key, delta_i64).await {
            Ok(new_value) => Ok(Ok(new_value as u64)),
            Err(e) => {
                tracing::error!("Redis error incrementing key: {}", e);
                Ok(Err(StoreError::Other(format!("Redis error: {}", e))))
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
    ) -> anyhow::Result<Result<Vec<Option<(String, Vec<u8>)>>, StoreError>> {
        let Some(plugin) = self.get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("get_many");

        let bucket_handle = self.table.get(&bucket)?;
        let mut conn = bucket_handle.conn.clone();

        if keys.is_empty() {
            return Ok(Ok(vec![]));
        }

        let redis_keys: Vec<String> = keys.iter().map(|k| bucket_handle.prefixed_key(k)).collect();

        let values: Vec<Option<Vec<u8>>> = match conn.mget(redis_keys.as_slice()).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("Redis error getting keys: {}", e);
                return Ok(Err(StoreError::Other(format!("Redis error: {}", e))));
            }
        };

        let result = keys
            .into_iter()
            .zip(values)
            .map(|(key, value)| value.map(|v| (key, v)))
            .collect();

        Ok(Ok(result))
    }

    async fn set_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<Result<(), StoreError>> {
        let Some(plugin) = self.get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("set_many");

        let bucket_handle = self.table.get(&bucket)?;
        let mut conn = bucket_handle.conn.clone();

        if key_values.is_empty() {
            return Ok(Ok(()));
        }

        let pairs: Vec<(String, Vec<u8>)> = key_values
            .into_iter()
            .map(|(key, value)| (bucket_handle.prefixed_key(&key), value))
            .collect();

        match conn.mset::<_, _, ()>(pairs.as_slice()).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("Redis error setting keys: {}", e);
                Ok(Err(StoreError::Other(format!("Redis error: {}", e))))
            }
        }
    }

    async fn delete_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        keys: Vec<String>,
    ) -> anyhow::Result<Result<(), StoreError>> {
        let Some(plugin) = self.get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };
        plugin.record_operation("delete_many");

        let bucket_handle = self.table.get(&bucket)?;

        let values = futures::stream::FuturesOrdered::from_iter(keys.iter().map(|key| {
            let mut conn = bucket_handle.conn.clone();
            let redis_key = bucket_handle.prefixed_key(key);
            async move {
                match conn.del::<_, ()>(redis_key).await {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        tracing::error!("Redis error deleting key: {}", e);
                        Err(StoreError::Other(format!("Redis error: {}", e)))
                    }
                }
            }
        }))
        .collect::<Vec<_>>()
        .await;

        for entry in values {
            if let Err(e) = entry {
                return Ok(Err(e));
            }
        }

        Ok(Ok(()))
    }
}

#[async_trait::async_trait]
impl HostPlugin for RedisKeyValue {
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
