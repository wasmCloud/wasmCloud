//! Redis-backed [`KvBackend`] for the multiplexed keyvalue plugin.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;

use crate::plugin::PluginEgressPolicy;
use crate::plugin::multiplex::BackendProvider;
use crate::plugin::wasi_keyvalue::redis::{Connection, ConnectionSettings};

use super::{KeyResponse, KvBackend, KvId, LIST_KEYS_BATCH_SIZE, StoreError};

/// A redis-backed [`KvBackend`] over a **flat keyspace**, so it interoperates
/// with a redis dataset created or managed outside this host:
/// `get(key)` is `GET key`, `increment` is `INCRBY`, etc.
///
/// Isolation between logical stores is by connection/DB — the `(implements ..)`
/// label selects the redis URL (and DB, via the url) — or an optional,
/// operator-set [`prefix`](Self::prefix). The `open(identifier)` bucket name does
/// NOT namespace keys, so a guest can open an externally-created keyspace simply
/// by pointing its label at the right redis. Holds a shared, self-healing
/// connection (pooled per url, prefix and connection settings by the provider):
/// see `wasi_keyvalue::redis::Connection`.
pub struct RedisBackend {
    conn: Connection,
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
        self.conn
            .query_read(redis::cmd("GET").arg(self.key(key)))
            .await
            .map_err(Self::err)
    }

    async fn set(&self, _bucket: &str, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        self.conn
            .query(redis::cmd("SET").arg(self.key(key)).arg(value))
            .await
            .map_err(Self::err)
    }

    async fn delete(&self, _bucket: &str, key: &str) -> Result<(), StoreError> {
        self.conn
            .query(redis::cmd("DEL").arg(self.key(key)))
            .await
            .map_err(Self::err)
    }

    async fn exists(&self, _bucket: &str, key: &str) -> Result<bool, StoreError> {
        self.conn
            .query_read(redis::cmd("EXISTS").arg(self.key(key)))
            .await
            .map_err(Self::err)
    }

    async fn list_keys(
        &self,
        _bucket: &str,
        cursor: Option<u64>,
    ) -> Result<KeyResponse, StoreError> {
        // SCAN the connection's keyspace under the (escaped) operator prefix.
        // With no prefix the pattern is `*`. The whole DB follows
        // flat-keyspace semantics; the guest's `list-keys` prefix is applied
        // host-side, so it never reaches this pattern.
        let prefix = self.prefix.as_deref().unwrap_or("");
        let pattern = format!("{}*", Self::glob_escape(prefix));
        // The cursor the guest sees carries the redis cursor and the
        // generation of the connection that issued it, and is refused after
        // a reconnect (see `Connection::scan_page`).
        let (next, raw): (u64, Vec<Vec<u8>>) = self
            .conn
            .scan_page::<Vec<u8>>(cursor.unwrap_or(0), &pattern, LIST_KEYS_BATCH_SIZE)
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
        // decrements); it errors on overflow, which we propagate. It is sent
        // once: if the connection drops under it, it may already have run, so
        // a retry could add twice. The error surfaces instead.
        self.conn
            .query(redis::cmd("INCRBY").arg(self.key(key)).arg(delta))
            .await
            .map_err(Self::err)
    }

    async fn get_many(
        &self,
        _bucket: &str,
        keys: Vec<String>,
    ) -> Result<Vec<Option<(String, Vec<u8>)>>, StoreError> {
        if keys.is_empty() {
            return Ok(vec![]);
        }
        let redis_keys: Vec<String> = keys.iter().map(|k| self.key(k)).collect();
        // Explicit MGET: the `mget` helper downgrades to `GET` for a single key,
        // and an absent single key then returns `nil`, which decodes to an
        // *empty* vec. MGET always returns one slot per key (nil → `None`).
        let values: Vec<Option<Vec<u8>>> = self
            .conn
            .query_read(redis::cmd("MGET").arg(redis_keys.as_slice()))
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
        let pairs: Vec<(String, Vec<u8>)> = key_values
            .into_iter()
            .map(|(k, v)| (self.key(&k), v))
            .collect();
        self.conn
            .query(redis::cmd("MSET").arg(pairs.as_slice()))
            .await
            .map_err(Self::err)
    }

    async fn delete_many(&self, _bucket: &str, keys: Vec<String>) -> Result<(), StoreError> {
        if keys.is_empty() {
            return Ok(());
        }
        let redis_keys: Vec<String> = keys.iter().map(|k| self.key(k)).collect();
        self.conn
            .query(redis::cmd("DEL").arg(redis_keys.as_slice()))
            .await
            .map_err(Self::err)
    }

    async fn put_if_absent(
        &self,
        _bucket: &str,
        key: &str,
        value: Vec<u8>,
    ) -> Result<bool, StoreError> {
        // SETNX is an atomic insert-if-absent. Sent once, like INCRBY: if the
        // first attempt ran before the connection dropped, a retry would
        // report the key as already present.
        self.conn
            .query(redis::cmd("SETNX").arg(self.key(key)).arg(value))
            .await
            .map_err(Self::err)
    }
}

/// Provider for [`RedisBackend`], selected by `config.backend = "redis"`.
/// Requires `config.url` (e.g. `redis://127.0.0.1:6379`); optional `config.prefix`
/// namespaces keys within the connection's keyspace (default none → flat).
/// The optional `backend-reconnect-num-retries`, `backend-reconnect-max-delay-ms`,
/// `backend-connection-timeout-ms` and `backend-response-timeout-ms` tune the
/// connection (see `ConnectionSettings`). All of these are operator-owned.
#[derive(Default)]
pub struct RedisProvider;

#[async_trait::async_trait]
impl BackendProvider<KvId> for RedisProvider {
    fn pool_key(&self, config: &HashMap<String, String>) -> Option<String> {
        // Pool by url AND prefix: two interfaces with the same url but different
        // prefixes are different logical stores and must not share an instance.
        // The connection settings are part of the key too, so a binding with a
        // tighter timeout never inherits another's connection. They go in
        // parsed, so two spellings of the same settings share one. A config
        // whose settings do not parse is not pooled; `instantiate` reports why.
        // The key never leaves this process, so the `Debug` rendering will do.
        let url = config.get("url")?;
        let prefix = config.get("prefix").map(String::as_str).unwrap_or("");
        let settings = ConnectionSettings::from_config(config).ok()?;
        Some(format!("{url}\u{0}{prefix}\u{0}{settings:?}"))
    }
    fn backend_type(&self) -> &'static str {
        "redis"
    }

    async fn instantiate(
        &self,
        config: &HashMap<String, String>,
        _egress: Option<&PluginEgressPolicy>,
    ) -> anyhow::Result<KvId> {
        let url = config
            .get("url")
            .ok_or_else(|| anyhow::anyhow!("redis keyvalue backend requires a 'url' config"))?;
        let settings = ConnectionSettings::from_config(config)?;
        let conn = Connection::connect(redis::Client::open(url.as_str())?, settings).await?;
        let prefix = config.get("prefix").filter(|p| !p.is_empty()).cloned();
        Ok(Arc::new(RedisBackend { conn, prefix }))
    }

    /// Checks the address `redis` itself parses out of the url. A unix socket
    /// has no host for `allowedHosts` to name, so it is refused.
    fn check_egress(
        &self,
        config: &HashMap<String, String>,
        policy: &PluginEgressPolicy,
    ) -> anyhow::Result<()> {
        let Some(url) = config.get("url") else {
            return Ok(());
        };
        let info = redis::IntoConnectionInfo::into_connection_info(url.as_str())
            .context("redis url is invalid")?;
        match info.addr {
            redis::ConnectionAddr::Tcp(host, port) => policy.check_host_port("redis", &host, port),
            redis::ConnectionAddr::TcpTls { host, port, .. } => {
                policy.check_host_port("rediss", &host, port)
            }
            redis::ConnectionAddr::Unix(path) => anyhow::bail!(
                "redis endpoint {path:?} is a unix socket, which allowedHosts cannot permit"
            ),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn check(url: &str, policy: &PluginEgressPolicy) -> anyhow::Result<()> {
        RedisProvider.check_egress(
            &HashMap::from([("url".to_string(), url.to_string())]),
            policy,
        )
    }

    #[test]
    fn egress_is_checked_at_the_address_redis_dials() {
        let policy =
            PluginEgressPolicy::allowing(&["redis://cache.internal:6379", "[fd00::5]:6379"]);
        check("redis://cache.internal", &policy).expect("the default port is 6379");
        check("redis://:secret@cache.internal:6379/2", &policy).unwrap();
        check("redis://[fd00::5]:6379", &policy).unwrap();

        let err = check("redis://evil.example:6379", &policy).unwrap_err();
        assert!(format!("{err:#}").contains("evil.example"), "{err:#}");
        assert!(check("redis://cache.internal:6380", &policy).is_err());
        assert!(check("redis+unix:///var/run/redis.sock", &policy).is_err());
    }

    fn cfg(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn bindings_with_different_connection_settings_do_not_share_a_connection() {
        let base = [("url", "redis://h:6379"), ("prefix", "p")];
        let plain = RedisProvider.pool_key(&cfg(&base)).unwrap();
        let tight = RedisProvider
            .pool_key(&cfg(&[
                base[0],
                base[1],
                ("backend-response-timeout-ms", "50"),
            ]))
            .unwrap();
        let tight_v1 = RedisProvider
            .pool_key(&cfg(&[
                base[0],
                base[1],
                ("BACKEND_RESPONSE_TIMEOUT_MS", "50"),
            ]))
            .unwrap();
        assert_ne!(plain, tight);
        assert_eq!(tight, tight_v1, "a spelling is not a different setting");

        // Settings that do not parse are never pooled; instantiate says why.
        let bad = cfg(&[base[0], ("backend-connection-timeout-ms", "soon")]);
        assert_eq!(RedisProvider.pool_key(&bad), None);
    }

    #[tokio::test]
    async fn invalid_connection_settings_fail_instantiate_before_connecting() {
        // Nothing listens on the url; the refusal must come from the config.
        let err = RedisProvider
            .instantiate(
                &cfg(&[
                    ("url", "redis://127.0.0.1:1"),
                    ("backend-reconnect-num-retries", "lots"),
                ]),
                None,
            )
            .await
            .err()
            .expect("an invalid setting is refused")
            .to_string();
        assert!(err.contains("backend-reconnect-num-retries"), "{err}");
    }

    /// A workload under `workloadConfig: deny` cannot set a connection setting,
    /// even when the operator lists no `hostOwnedKeys`: the plugin claims them.
    #[test]
    fn deny_refuses_a_workload_that_sets_a_connection_setting() {
        use crate::plugin::{HostPlugin, PluginBindingSet, WorkloadConfigPolicy};

        let plugins: [Box<dyn HostPlugin>; 2] = [
            Box::new(crate::plugin::wasi_keyvalue::MultiplexedKeyValue::new()),
            Box::new(crate::plugin::wasi_keyvalue::MultiplexedAsyncKeyValue::new()),
        ];
        for plugin in plugins {
            let set = PluginBindingSet::new(plugin.id())
                .with_binding("cache", cfg(&[("url", "redis://cache.internal:6379")]))
                .with_workload_config(WorkloadConfigPolicy::Deny);
            let schema = plugin.binding_schema();
            for key in [
                "backend-reconnect-num-retries",
                "BACKEND_RECONNECT_MAX_DELAY_MS",
                "backend_connection_timeout_ms",
                "backend-response-timeout-ms",
            ] {
                let err = set
                    .resolve(
                        "cache",
                        &cfg(&[("backend", "redis"), (key, "1")]),
                        &schema,
                        crate::plugin::bindings::never_narrows(),
                    )
                    .unwrap_err()
                    .to_string();
                assert!(err.contains(key), "{}: {err}", plugin.id());
                assert!(
                    err.contains("belong to the operator"),
                    "{}: {err}",
                    plugin.id()
                );
            }

            // The operator may still set them for the binding.
            let operator = PluginBindingSet::new(plugin.id())
                .with_binding(
                    "cache",
                    cfg(&[
                        ("url", "redis://cache.internal:6379"),
                        ("backend-response-timeout-ms", "250"),
                    ]),
                )
                .with_workload_config(WorkloadConfigPolicy::Deny);
            let resolved = operator
                .resolve(
                    "cache",
                    &cfg(&[("backend", "redis")]),
                    &schema,
                    crate::plugin::bindings::never_narrows(),
                )
                .unwrap();
            assert_eq!(resolved.get("backend-response-timeout-ms").unwrap(), "250");
        }
    }
}
