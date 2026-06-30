//! Redis-backed [`KvBackend`] for the multiplexed keyvalue plugin.

use std::collections::HashMap;
use std::sync::Arc;

use redis::AsyncCommands;

use crate::plugin::multiplex::BackendProvider;

use super::{KeyResponse, KvBackend, KvId, LIST_KEYS_BATCH_SIZE, StoreError};

/// A redis-backed [`KvBackend`] over a **flat keyspace**, so it interoperates
/// with a redis dataset created or managed outside this host:
/// `get(key)` is `GET key`, `increment` is `INCRBY`, etc.
///
/// Isolation between logical stores is by connection/DB — the `(implements ..)`
/// label selects the redis URL (and DB, via the url) — or an optional,
/// operator-set [`prefix`](Self::prefix). The `open(identifier)` bucket name does
/// NOT namespace keys, so a guest can open an externally-created keyspace simply
/// by pointing its label at the right redis. Holds a shared multiplexed
/// connection (pooled per url+prefix by the provider).
pub struct RedisBackend {
    conn: redis::aio::MultiplexedConnection,
    /// Optional operator-configured key prefix, default none.
    /// Lets a deployment namespace within a shared DB or match an external key
    /// convention. It is operator-set, not guest-controlled, so a guest cannot
    /// use it to inject `SCAN` glob metacharacters; it is escaped regardless.
    prefix: Option<String>,
}

impl RedisBackend {
    /// Map a logical key to its redis key (prepending the optional prefix).
    fn key(&self, key: &str) -> String {
        match &self.prefix {
            Some(p) => format!("{p}{key}"),
            None => key.to_string(),
        }
    }

    fn err(e: impl std::fmt::Display) -> StoreError {
        StoreError::Other(format!("Redis error: {e}"))
    }

    /// Escape redis glob metacharacters so a literal prefix matches verbatim in
    /// `SCAN MATCH`.
    fn glob_escape(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            if matches!(c, '*' | '?' | '[' | ']' | '\\' | '^') {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }
}

#[async_trait::async_trait]
impl KvBackend for RedisBackend {
    async fn open(&self, _identifier: &str) -> Result<(), StoreError> {
        // The bucket identifier does not namespace keys. The connection (label/url) and optional `prefix` do.
        Ok(())
    }

    async fn get(&self, _bucket: &str, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let mut conn = self.conn.clone();
        conn.get::<_, Option<Vec<u8>>>(self.key(key))
            .await
            .map_err(Self::err)
    }

    async fn set(&self, _bucket: &str, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        let mut conn = self.conn.clone();
        conn.set::<_, _, ()>(self.key(key), value)
            .await
            .map_err(Self::err)
    }

    async fn delete(&self, _bucket: &str, key: &str) -> Result<(), StoreError> {
        let mut conn = self.conn.clone();
        conn.del::<_, ()>(self.key(key)).await.map_err(Self::err)
    }

    async fn exists(&self, _bucket: &str, key: &str) -> Result<bool, StoreError> {
        let mut conn = self.conn.clone();
        conn.exists::<_, bool>(self.key(key))
            .await
            .map_err(Self::err)
    }

    async fn list_keys(
        &self,
        _bucket: &str,
        cursor: Option<u64>,
    ) -> Result<KeyResponse, StoreError> {
        let mut conn = self.conn.clone();
        // SCAN the connection's keyspace under the (escaped) operator prefix.
        // With no prefix the pattern is `*`. The whole DB follows
        // flat-keyspace semantics; the guest's `list-keys` prefix is applied
        // host-side, so it never reaches this pattern.
        let prefix = self.prefix.as_deref().unwrap_or("");
        let pattern = format!("{}*", Self::glob_escape(prefix));
        let (next, raw): (u64, Vec<Vec<u8>>) = redis::cmd("SCAN")
            .arg(cursor.unwrap_or(0))
            .arg("MATCH")
            .arg(&pattern)
            .arg("COUNT")
            .arg(LIST_KEYS_BATCH_SIZE)
            .query_async(&mut conn)
            .await
            .map_err(Self::err)?;
        let strip = prefix.as_bytes();
        let keys = raw
            .into_iter()
            .map(|k| {
                let logical = k.strip_prefix(strip).unwrap_or(&k);
                String::from_utf8_lossy(logical).into_owned()
            })
            .collect();
        Ok(KeyResponse {
            keys,
            cursor: (next != 0).then_some(next),
        })
    }

    async fn increment(&self, _bucket: &str, key: &str, delta: i64) -> Result<i64, StoreError> {
        // INCRBY is a native, signed, atomic increment (negative delta
        // decrements); it errors on overflow, which we propagate.
        let mut conn = self.conn.clone();
        conn.incr(self.key(key), delta).await.map_err(Self::err)
    }

    async fn get_many(
        &self,
        _bucket: &str,
        keys: Vec<String>,
    ) -> Result<Vec<Option<(String, Vec<u8>)>>, StoreError> {
        if keys.is_empty() {
            return Ok(vec![]);
        }
        let mut conn = self.conn.clone();
        let redis_keys: Vec<String> = keys.iter().map(|k| self.key(k)).collect();
        // Explicit MGET: the `mget` helper downgrades to `GET` for a single key,
        // and an absent single key then returns `nil`, which decodes to an
        // *empty* vec. MGET always returns one slot per key (nil → `None`).
        let values: Vec<Option<Vec<u8>>> = redis::cmd("MGET")
            .arg(redis_keys.as_slice())
            .query_async(&mut conn)
            .await
            .map_err(Self::err)?;
        Ok(keys
            .into_iter()
            .zip(values)
            .map(|(k, v)| v.map(|v| (k, v)))
            .collect())
    }

    async fn set_many(
        &self,
        _bucket: &str,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> Result<(), StoreError> {
        if key_values.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.clone();
        let pairs: Vec<(String, Vec<u8>)> = key_values
            .into_iter()
            .map(|(k, v)| (self.key(&k), v))
            .collect();
        conn.mset::<_, _, ()>(pairs.as_slice())
            .await
            .map_err(Self::err)
    }

    async fn delete_many(&self, _bucket: &str, keys: Vec<String>) -> Result<(), StoreError> {
        if keys.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.clone();
        let redis_keys: Vec<String> = keys.iter().map(|k| self.key(k)).collect();
        conn.del::<_, ()>(redis_keys.as_slice())
            .await
            .map_err(Self::err)
    }

    async fn put_if_absent(
        &self,
        _bucket: &str,
        key: &str,
        value: Vec<u8>,
    ) -> Result<bool, StoreError> {
        // SETNX is an atomic insert-if-absent.
        let mut conn = self.conn.clone();
        conn.set_nx::<_, _, bool>(self.key(key), value)
            .await
            .map_err(Self::err)
    }
}

/// Provider for [`RedisBackend`], selected by `config.backend = "redis"`.
/// Requires `config.url` (e.g. `redis://127.0.0.1:6379`); optional `config.prefix`
/// namespaces keys within the connection's keyspace (default none → flat).
#[derive(Default)]
pub struct RedisProvider;

#[async_trait::async_trait]
impl BackendProvider<KvId> for RedisProvider {
    fn pool_key(&self, config: &HashMap<String, String>) -> Option<String> {
        // Pool by url AND prefix: two interfaces with the same url but different
        // prefixes are different logical stores and must not share an instance.
        let url = config.get("url")?;
        let prefix = config.get("prefix").map(String::as_str).unwrap_or("");
        Some(format!("{url}\u{0}{prefix}"))
    }
    fn backend_type(&self) -> &'static str {
        "redis"
    }

    async fn instantiate(&self, config: &HashMap<String, String>) -> anyhow::Result<KvId> {
        let url = config
            .get("url")
            .ok_or_else(|| anyhow::anyhow!("redis keyvalue backend requires a 'url' config"))?;
        let client = redis::Client::open(url.as_str())?;
        let conn = client.get_multiplexed_async_connection().await?;
        let prefix = config.get("prefix").filter(|p| !p.is_empty()).cloned();
        Ok(Arc::new(RedisBackend { conn, prefix }))
    }
}
