//! # WASI KeyValue Redis Plugin
//!
//! This module implements `wasi:keyvalue@0.2.0-draft` interfaces using
//! Redis as the backend storage.
//! The `open` identifier is used as a key prefix (`{identifier}:{key}`) to
//! namespace keys within a single Redis database.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

const PLUGIN_KEYVALUE_ID: &str = "wasi-keyvalue";
use crate::engine::ctx::{ActiveCtx, SharedCtx, extract_active_ctx};
use crate::engine::workload::WorkloadItem;
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
            "wasi:keyvalue/store.bucket": crate::plugin::wasi_keyvalue::redis::BucketHandle,
        },
    });
}

use bindings::wasi::keyvalue::store::{Error as StoreError, KeyResponse};

/// Resource representation for a bucket (key-value store namespace)
pub struct BucketHandle {
    conn: Connection,
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
    /// One self-healing connection shared by every bucket, opened by the first
    /// `open` that succeeds. A failed open leaves it empty, so the next one
    /// tries again.
    conn: Arc<tokio::sync::OnceCell<Connection>>,
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
            conn: Arc::default(),
            metrics: Arc::new(metrics),
        }
    }

    pub fn from_url(url: &url::Url) -> anyhow::Result<Self> {
        let client = redis::Client::open(url.as_str())?;
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

/// `config` key: how many times a dropped connection is redialed before the
/// call waiting on it fails. The v1 redis provider's name for it.
#[cfg_attr(not(feature = "wasm_component_model_implements"), allow(dead_code))]
pub(crate) const RECONNECT_NUM_RETRIES_KEY: &str = "backend-reconnect-num-retries";
/// `config` key: the longest pause, in milliseconds, between redial attempts.
#[cfg_attr(not(feature = "wasm_component_model_implements"), allow(dead_code))]
pub(crate) const RECONNECT_MAX_DELAY_MS_KEY: &str = "backend-reconnect-max-delay-ms";
/// `config` key: how long, in milliseconds, one connection attempt may take.
#[cfg_attr(not(feature = "wasm_component_model_implements"), allow(dead_code))]
pub(crate) const CONNECTION_TIMEOUT_MS_KEY: &str = "backend-connection-timeout-ms";
/// `config` key: how long, in milliseconds, one command may wait for its reply.
#[cfg_attr(not(feature = "wasm_component_model_implements"), allow(dead_code))]
pub(crate) const RESPONSE_TIMEOUT_MS_KEY: &str = "backend-response-timeout-ms";

/// Every redis connection setting a binding's `config` may carry. Each is the
/// operator's: the multiplexed plugins' `KEYVALUE_HOST_OWNED_KEYS` claims
/// them all, so a workload under `workloadConfig: deny` cannot set one.
#[cfg(all(test, feature = "wasm_component_model_implements"))]
pub(crate) const CONNECTION_SETTING_KEYS: &[&str] = &[
    RECONNECT_NUM_RETRIES_KEY,
    RECONNECT_MAX_DELAY_MS_KEY,
    CONNECTION_TIMEOUT_MS_KEY,
    RESPONSE_TIMEOUT_MS_KEY,
];

/// Upper bound on [`RECONNECT_NUM_RETRIES_KEY`]. A call that finds the
/// connection dead waits for every redial, so this bounds that wait.
#[cfg_attr(not(feature = "wasm_component_model_implements"), allow(dead_code))]
const MAX_RECONNECT_NUM_RETRIES: u64 = 1_000;
/// Upper bound on each millisecond setting (ten minutes).
#[cfg_attr(not(feature = "wasm_component_model_implements"), allow(dead_code))]
const MAX_SETTING_MS: u64 = 600_000;

/// How a redis keyvalue backend dials, redials, and waits on its server.
///
/// The defaults are the v1 redis provider's: a bounded reconnect, so a call
/// made while the server is down fails within about a second rather than after
/// the crate default's minutes of backoff, and a three second cap on each
/// connection attempt. The keys keep v1's names (any case, `_` or `-`), so
/// `BACKEND_CONNECTION_TIMEOUT_MS` from a v1 link reads the same here.
///
/// There is no response timeout by default. A `SCAN` page or an `MGET` of many
/// large values can legitimately take a while, and a timed-out command is not
/// cancelled on the server, so a default that is too tight turns slow successes
/// into errors that still had their effect. An operator who wants one sets
/// [`RESPONSE_TIMEOUT_MS_KEY`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ConnectionSettings {
    reconnect_num_retries: usize,
    reconnect_max_delay_ms: u64,
    connection_timeout_ms: u64,
    response_timeout_ms: Option<u64>,
}

impl Default for ConnectionSettings {
    fn default() -> Self {
        Self {
            reconnect_num_retries: 3,
            reconnect_max_delay_ms: 300,
            connection_timeout_ms: 3_000,
            response_timeout_ms: None,
        }
    }
}

impl ConnectionSettings {
    /// Read the settings from a binding's `config`, defaulting what is unset.
    ///
    /// Keys match in any spelling [`crate::plugin::bindings::canonical_key`] folds
    /// together, the same rule `workloadConfig: deny` uses to refuse them.
    ///
    /// # Errors
    ///
    /// A value that is not a whole number in range, a timeout of zero, or one
    /// key given twice under different spellings with different values.
    #[cfg_attr(not(feature = "wasm_component_model_implements"), allow(dead_code))]
    pub(crate) fn from_config(config: &HashMap<String, String>) -> anyhow::Result<Self> {
        let mut settings = Self::default();
        if let Some(v) = setting(config, RECONNECT_NUM_RETRIES_KEY)? {
            let retries =
                parse_bounded(RECONNECT_NUM_RETRIES_KEY, v, 0, MAX_RECONNECT_NUM_RETRIES)?;
            settings.reconnect_num_retries = usize::try_from(retries)?;
        }
        if let Some(v) = setting(config, RECONNECT_MAX_DELAY_MS_KEY)? {
            settings.reconnect_max_delay_ms =
                parse_bounded(RECONNECT_MAX_DELAY_MS_KEY, v, 0, MAX_SETTING_MS)?;
        }
        if let Some(v) = setting(config, CONNECTION_TIMEOUT_MS_KEY)? {
            settings.connection_timeout_ms =
                parse_bounded(CONNECTION_TIMEOUT_MS_KEY, v, 1, MAX_SETTING_MS)?;
        }
        if let Some(v) = setting(config, RESPONSE_TIMEOUT_MS_KEY)? {
            settings.response_timeout_ms = Some(parse_bounded(
                RESPONSE_TIMEOUT_MS_KEY,
                v,
                1,
                MAX_SETTING_MS,
            )?);
        }
        Ok(settings)
    }

    /// A stable rendering of the parsed values, so two bindings that spell the
    /// same settings differently still share one pooled connection, and two
    /// with different settings never do.
    #[cfg_attr(not(feature = "wasm_component_model_implements"), allow(dead_code))]
    pub(crate) fn pool_key_suffix(&self) -> String {
        let response = self
            .response_timeout_ms
            .map_or_else(|| "none".to_string(), |ms| ms.to_string());
        format!(
            "retries={};max-delay-ms={};connect-ms={};response-ms={response}",
            self.reconnect_num_retries, self.reconnect_max_delay_ms, self.connection_timeout_ms
        )
    }

    fn connection_config(&self) -> redis::AsyncConnectionConfig {
        let mut config = redis::AsyncConnectionConfig::new()
            .set_connection_timeout(Duration::from_millis(self.connection_timeout_ms));
        if let Some(ms) = self.response_timeout_ms {
            config = config.set_response_timeout(Duration::from_millis(ms));
        }
        config
    }

    /// The pause before redial `attempt` (counting from zero): between half
    /// and all of `min(max delay, 50ms * 2^attempt)`. The jitter keeps many
    /// pooled connections that lost one server from redialing in lockstep.
    fn redial_delay(&self, attempt: usize) -> Duration {
        let exponent = u32::try_from(attempt).unwrap_or(u32::MAX).min(16);
        let ceiling = REDIAL_BASE_DELAY_MS
            .saturating_mul(1 << exponent)
            .min(self.reconnect_max_delay_ms);
        Duration::from_millis(fastrand::u64(ceiling / 2..=ceiling))
    }
}

/// The first redial pause, before doubling; see
/// [`ConnectionSettings::redial_delay`].
const REDIAL_BASE_DELAY_MS: u64 = 50;

/// The value of `key` in `config`, in any spelling that folds to it.
#[cfg_attr(not(feature = "wasm_component_model_implements"), allow(dead_code))]
fn setting<'a>(config: &'a HashMap<String, String>, key: &str) -> anyhow::Result<Option<&'a str>> {
    let mut found: Option<(&str, &str)> = None;
    for (k, v) in config {
        if crate::plugin::bindings::canonical_key(k) != key {
            continue;
        }
        match found {
            Some((first, value)) if value != v.as_str() => anyhow::bail!(
                "redis keyvalue config sets `{key}` twice with different values \
                 (`{first}` and `{k}`)"
            ),
            _ => found = Some((k.as_str(), v.as_str())),
        }
    }
    Ok(found.map(|(_, v)| v))
}

#[cfg_attr(not(feature = "wasm_component_model_implements"), allow(dead_code))]
fn parse_bounded(key: &str, value: &str, min: u64, max: u64) -> anyhow::Result<u64> {
    let parsed: u64 = value.trim().parse().map_err(|_| {
        anyhow::anyhow!("redis keyvalue config `{key}` must be a whole number, got {value:?}")
    })?;
    anyhow::ensure!(
        (min..=max).contains(&parsed),
        "redis keyvalue config `{key}` must be between {min} and {max}, got {parsed}"
    );
    Ok(parsed)
}

/// A redis connection that replaces itself when it dies, shared by every
/// call on a backend.
///
/// Both redis keyvalue plugins hold a connection longer than one call (the
/// multiplexed backend is pooled and outlives any one workload; the
/// single-backend plugin shares one across every bucket it opens), so a bare
/// `MultiplexedConnection` that dies with the server (a restart, a failover)
/// would fail every later call until the host restarts. This one drops a
/// connection that a command found dead, and the next command dials a new one.
/// A read that found it dead is sent once more on the replacement (see
/// [`Connection::query_read`]); a write may already have run, so its call
/// fails and the calls after it use the fresh connection.
///
/// Every dial uses the same `client`, so the same host and port that the
/// multiplexed provider's `check_egress` approved, resolved by name exactly as
/// the first connect was, with the same credentials and DB.
///
/// The first connect is one attempt, so a server that is down, a wrong url or
/// a bad password fails a bind at once. A redial makes up to the configured
/// number of further attempts, pausing as [`ConnectionSettings`] says, and
/// concurrent callers wait on that one redial rather than each dialing. A
/// redial that fails leaves no connection behind, so the next call starts a
/// fresh one: no failure, of any kind, is kept.
///
/// This is not redis's own `ConnectionManager`: in redis 0.27 its first
/// backoff pause ignores the configured maximum (one to two seconds whatever
/// the setting), its initial connect cannot be made to fail fast, and a redial
/// that fails with anything but an I/O error (an `AUTH` refusal, or `LOADING`
/// while a restarted server reads its dataset) is kept and returned to every
/// later call, with no further redial, until the host restarts.
#[derive(Clone)]
pub(crate) struct Connection {
    inner: Arc<ConnectionInner>,
}

struct ConnectionInner {
    client: redis::Client,
    settings: ConnectionSettings,
    /// The live connection and the generation that dialed it, or `None` once
    /// a command found it dead.
    current: std::sync::Mutex<Option<(redis::aio::MultiplexedConnection, u64)>>,
    /// Held while redialing, so one redial serves every caller waiting on it.
    dialing: tokio::sync::Mutex<Redials>,
    /// How many redials have finished, read without the lock so a caller can
    /// tell whether one finished while it waited.
    redials_finished: std::sync::atomic::AtomicU64,
}

/// A list-keys cursor this backend handed out has this bit set, the
/// generation of the connection that issued it in the bits below, and the
/// redis `SCAN` cursor in the low [`CURSOR_BITS`]. See
/// [`Connection::scan_page`].
const TOKEN_FLAG: u64 = 1 << 63;
/// Bits of a redis `SCAN` cursor a token carries. A redis cursor indexes the
/// keyspace hash table, which never approaches 2^48 buckets.
const CURSOR_BITS: u32 = 48;
const CURSOR_MASK: u64 = (1 << CURSOR_BITS) - 1;
/// Bits of the connection generation a token carries (it wraps).
const GENERATION_MASK: u64 = (1 << (63 - CURSOR_BITS)) - 1;

#[derive(Default)]
struct Redials {
    /// Connections dialed so far, used as their generation.
    dialed: u64,
    /// The last redial's failure, tagged with the `redials_finished` count it
    /// set, or `None` once a redial succeeds.
    failed: Option<(u64, Arc<redis::RedisError>)>,
}

/// Why a [`Connection`] call failed: no connection could be made, or the
/// command failed on one.
enum Failure {
    Dial(redis::RedisError),
    Command(redis::RedisError),
}

impl Failure {
    fn into_inner(self) -> redis::RedisError {
        match self {
            Self::Dial(e) | Self::Command(e) => e,
        }
    }
}

impl Connection {
    /// Connect to the server `client` names, in one attempt.
    pub(crate) async fn connect(
        client: redis::Client,
        settings: ConnectionSettings,
    ) -> redis::RedisResult<Self> {
        let conn = client
            .get_multiplexed_async_connection_with_config(&settings.connection_config())
            .await?;
        Ok(Self {
            inner: Arc::new(ConnectionInner {
                client,
                settings,
                current: std::sync::Mutex::new(Some((conn, 0))),
                dialing: tokio::sync::Mutex::default(),
                redials_finished: std::sync::atomic::AtomicU64::new(0),
            }),
        })
    }

    fn live(&self) -> Option<(redis::aio::MultiplexedConnection, u64)> {
        self.inner
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The live connection, redialing first if a command found it dead.
    ///
    /// A caller that waited while another redialed takes that redial's
    /// outcome, failure included, rather than starting its own: during an
    /// outage, a queue of callers fails together after one redial schedule
    /// instead of each in turn after its own. A caller that arrives after a
    /// redial failed starts a fresh one.
    async fn get(&self) -> redis::RedisResult<(redis::aio::MultiplexedConnection, u64)> {
        use std::sync::atomic::Ordering;

        if let Some(live) = self.live() {
            return Ok(live);
        }
        let seen = self.inner.redials_finished.load(Ordering::Acquire);
        let mut redials = self.inner.dialing.lock().await;
        if let Some(live) = self.live() {
            return Ok(live);
        }
        if let Some((finished, e)) = &redials.failed
            && *finished > seen
        {
            return Err(redis::RedisError::from((
                e.kind(),
                "redis redial failed",
                e.to_string(),
            )));
        }

        let settings = &self.inner.settings;
        let config = settings.connection_config();
        let mut attempt = 0;
        let outcome = loop {
            match self
                .inner
                .client
                .get_multiplexed_async_connection_with_config(&config)
                .await
            {
                Ok(conn) => break Ok(conn),
                Err(e) if attempt >= settings.reconnect_num_retries => break Err(e),
                Err(e) => {
                    tracing::debug!(error = %e, attempt, "redis redial failed; retrying");
                    tokio::time::sleep(settings.redial_delay(attempt)).await;
                    attempt += 1;
                }
            }
        };
        let finished = self.inner.redials_finished.load(Ordering::Acquire) + 1;
        let result = match outcome {
            Ok(conn) => {
                redials.dialed += 1;
                redials.failed = None;
                let live = (conn, redials.dialed);
                *self
                    .inner
                    .current
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(live.clone());
                Ok(live)
            }
            Err(e) => {
                let e = Arc::new(e);
                redials.failed = Some((finished, e.clone()));
                Err(redis::RedisError::from((
                    e.kind(),
                    "redis redial failed",
                    e.to_string(),
                )))
            }
        };
        self.inner
            .redials_finished
            .store(finished, Ordering::Release);
        result
    }

    /// Forget connection `generation` if it is still the live one.
    fn discard(&self, generation: u64) {
        let mut current = self
            .inner
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current.as_ref().is_some_and(|(_, g)| *g == generation) {
            *current = None;
        }
    }

    /// Send `cmd` once, telling a failure to connect from a failed command.
    /// A command that proved the connection dead drops it, and the next
    /// command redials.
    async fn send<T: redis::FromRedisValue>(&self, cmd: &redis::Cmd) -> Result<T, Failure> {
        let (mut conn, generation) = self.get().await.map_err(Failure::Dial)?;
        cmd.query_async(&mut conn).await.map_err(|e| {
            if e.is_unrecoverable_error() {
                self.discard(generation);
            }
            Failure::Command(e)
        })
    }

    /// Send `cmd` once. If the connection proved dead, it is dropped, and the
    /// next command redials.
    pub(crate) async fn query<T: redis::FromRedisValue>(
        &self,
        cmd: &redis::Cmd,
    ) -> redis::RedisResult<T> {
        self.send(cmd).await.map_err(Failure::into_inner)
    }

    /// Send a read, and once more if the connection dropped under the first
    /// attempt (see [`is_dropped_connection`]).
    ///
    /// Only for reads (`GET`, `MGET`, `EXISTS`): running one twice changes
    /// nothing. Writes are sent once with [`Connection::query`], even `SET`,
    /// `MSET` and `DEL`, which leave the same state however often they run on
    /// their own: if the first attempt was applied and only its reply was lost
    /// with the connection, a replay could land after another client's write
    /// to the same key and overwrite or delete it. So a write that finds the
    /// connection dead returns the error, and the caller decides whether to
    /// try again. `INCRBY` and `SET NX` are never retried for the same reason
    /// and more (a second `INCRBY` adds twice).
    ///
    /// Only a command that failed on a connection is retried. If no connection
    /// could be made, the redial already spent its retries, and the error is
    /// returned as it is.
    ///
    /// The retry redials, so after a restart the first read succeeds rather
    /// than failing once per pooled connection.
    pub(crate) async fn query_read<T: redis::FromRedisValue>(
        &self,
        cmd: &redis::Cmd,
    ) -> redis::RedisResult<T> {
        match self.send(cmd).await {
            Err(Failure::Command(e)) if is_dropped_connection(&e) => {
                tracing::debug!(error = %e, "redis connection dropped; retrying a read once");
                self.query(cmd).await
            }
            other => other.map_err(Failure::into_inner),
        }
    }

    /// Fetch one `SCAN` page of keys matching `pattern`, as `(next cursor,
    /// keys)`, where a cursor of zero starts and ends a listing.
    ///
    /// A redis cursor names a position in the hash table of the server that
    /// issued it, and a restarted or failed-over server may lay its table out
    /// differently, so a redis cursor replayed there could silently skip keys.
    /// So the cursors handed out are tokens that carry the generation of the
    /// connection that issued them beside the redis cursor (see
    /// [`TOKEN_FLAG`]), and a token from before the latest redial (however
    /// that redial came about, a `get` between pages included) is refused,
    /// telling the caller to list again from the first page. Nothing is
    /// remembered per token, so no caller's listing can crowd out another's.
    ///
    /// A cursor without [`TOKEN_FLAG`] (one from before tokens, or a redis
    /// cursor too large to carry, which no redis keyspace produces) is sent to
    /// redis as it is, unchecked.
    ///
    /// A page whose connection dropped under it is retried once like any read.
    /// For a first page that is a fresh scan; for a later one the retry
    /// redials, and its token is then refused as above.
    pub(crate) async fn scan_page<K: redis::FromRedisValue>(
        &self,
        token: u64,
        pattern: &str,
        count: usize,
    ) -> redis::RedisResult<(u64, Vec<K>)> {
        let mut retried = false;
        loop {
            let (mut conn, generation) = self.get().await?;
            let cursor = if token & TOKEN_FLAG == 0 {
                token
            } else if (token >> CURSOR_BITS) & GENERATION_MASK == generation & GENERATION_MASK {
                token & CURSOR_MASK
            } else {
                return Err(redis::RedisError::from((
                    redis::ErrorKind::ClientError,
                    "list-keys cursor is from before a reconnect",
                    "the server may have changed; list again from the first page".to_string(),
                )));
            };
            let scan = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(count)
                .clone();
            match scan.query_async::<(u64, Vec<K>)>(&mut conn).await {
                Ok((next, keys)) => {
                    let next = if next == 0 || next > CURSOR_MASK {
                        next
                    } else {
                        TOKEN_FLAG | ((generation & GENERATION_MASK) << CURSOR_BITS) | next
                    };
                    return Ok((next, keys));
                }
                Err(e) => {
                    if e.is_unrecoverable_error() {
                        self.discard(generation);
                    }
                    if !retried && is_dropped_connection(&e) {
                        tracing::debug!(error = %e, "redis connection dropped; retrying a SCAN page once");
                        retried = true;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }
}

/// Whether `e` says the connection died under the command, as when the
/// server restarts: a broken pipe, a reset or aborted connection, or an EOF.
///
/// A refused connection or a timeout is not one. A refusal reaches a call
/// only after a redial's bounded attempts have failed, so retrying it would
/// just run that schedule again and double the time a call takes to fail
/// while the server is down. A timeout (only with a response timeout set)
/// means the server is slow, not gone, and retrying would double the wait.
pub(crate) fn is_dropped_connection(e: &redis::RedisError) -> bool {
    e.is_io_error() && e.is_unrecoverable_error() && !e.is_connection_refusal()
}

// Implementation for the store interface
impl<'a> bindings::wasi::keyvalue::store::Host for ActiveCtx<'a> {
    #[instrument(name = "wasi.keyvalue.open", skip(self))]
    async fn open(
        &mut self,
        identifier: String,
    ) -> wasmtime::Result<Result<Resource<BucketHandle>, StoreError>> {
        let plugin = self.try_get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("open");

        // Every bucket shares one multiplexed connection that replaces itself
        // if the server restarts, rather than dialing (and holding) a fresh
        // connection per `open`: a guest that opens a bucket per request would
        // otherwise pay a connect each time.
        let conn = match plugin
            .conn
            .get_or_try_init(|| {
                Connection::connect(plugin.client.clone(), ConnectionSettings::default())
            })
            .await
        {
            Ok(conn) => conn.clone(),
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
    #[instrument(name = "wasi.keyvalue.get", skip(self, bucket))]
    async fn get(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<Option<Vec<u8>>, StoreError>> {
        let plugin = self.try_get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("get");

        let bucket_handle = self.table.get(&bucket)?;
        let conn = bucket_handle.conn.clone();
        let redis_key = bucket_handle.prefixed_key(&key);

        match conn
            .query_read::<Option<Vec<u8>>>(redis::cmd("GET").arg(redis_key))
            .await
        {
            Ok(value) => Ok(Ok(value)),
            Err(e) => {
                tracing::error!("Redis error getting key: {}", e);
                Ok(Err(StoreError::Other(format!("Redis error: {e}"))))
            }
        }
    }

    #[instrument(name = "wasi.keyvalue.set", skip(self, bucket, value))]
    async fn set(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let plugin = self.try_get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("set");

        let bucket_handle = self.table.get(&bucket)?;
        let conn = bucket_handle.conn.clone();
        let redis_key = bucket_handle.prefixed_key(&key);

        match conn
            .query::<()>(redis::cmd("SET").arg(redis_key).arg(value))
            .await
        {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("Redis error setting key: {}", e);
                Ok(Err(StoreError::Other(format!("Redis error: {e}"))))
            }
        }
    }

    #[instrument(name = "wasi.keyvalue.delete", skip(self, bucket))]
    async fn delete(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let plugin = self.try_get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("delete");

        let bucket_handle = self.table.get(&bucket)?;
        let conn = bucket_handle.conn.clone();
        let redis_key = bucket_handle.prefixed_key(&key);

        match conn.query::<()>(redis::cmd("DEL").arg(redis_key)).await {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("Redis error deleting key: {}", e);
                Ok(Err(StoreError::Other(format!("Redis error: {e}"))))
            }
        }
    }

    #[instrument(name = "wasi.keyvalue.exists", skip(self, bucket))]
    async fn exists(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> wasmtime::Result<Result<bool, StoreError>> {
        let plugin = self.try_get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("exists");

        let bucket_handle = self.table.get(&bucket)?;
        let conn = bucket_handle.conn.clone();
        let redis_key = bucket_handle.prefixed_key(&key);

        match conn
            .query_read::<bool>(redis::cmd("EXISTS").arg(redis_key))
            .await
        {
            Ok(exists) => Ok(Ok(exists)),
            Err(e) => {
                tracing::error!("Redis error checking key existence: {}", e);
                Ok(Err(StoreError::Other(format!("Redis error: {e}"))))
            }
        }
    }

    #[instrument(name = "wasi.keyvalue.list_keys", skip(self, bucket))]
    async fn list_keys(
        &mut self,
        bucket: Resource<BucketHandle>,
        cursor: Option<u64>,
    ) -> wasmtime::Result<Result<KeyResponse, StoreError>> {
        let plugin = self.try_get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("list_keys");

        let bucket_handle = self.table.get(&bucket)?;
        let conn = bucket_handle.conn.clone();
        let pattern = bucket_handle.prefixed_key("*");
        let redis_cursor = cursor.unwrap_or(0);

        let (next_cursor, raw_keys): (u64, Vec<String>) = match conn
            .scan_page::<String>(redis_cursor, &pattern, LIST_KEYS_BATCH_SIZE)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                tracing::error!("Redis error listing keys: {}", e);
                return Ok(Err(StoreError::Other(format!("Redis error: {e}"))));
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
// Atomics use Redis' native INCRBY for atomic increment, storing values as
// Redis integer strings rather than big-endian bytes.
impl<'a> bindings::wasi::keyvalue::atomics::Host for ActiveCtx<'a> {
    #[instrument(name = "wasi.keyvalue.increment", skip(self, bucket))]
    async fn increment(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        delta: u64,
    ) -> wasmtime::Result<Result<u64, StoreError>> {
        let plugin = self.try_get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("increment");

        let bucket_handle = self.table.get(&bucket)?;
        let conn = bucket_handle.conn.clone();
        let redis_key = bucket_handle.prefixed_key(&key);

        // Sent once: if the connection drops under it, INCRBY may already have
        // run, so a retry could add twice. The error surfaces instead.
        let delta_i64 = i64::try_from(delta)
            .map_err(|_| wasmtime::format_err!("delta value {} exceeds i64::MAX", delta))?;
        match conn
            .query::<i64>(redis::cmd("INCRBY").arg(redis_key).arg(delta_i64))
            .await
        {
            Ok(new_value) => Ok(Ok(new_value as u64)),
            Err(e) => {
                tracing::error!("Redis error incrementing key: {}", e);
                Ok(Err(StoreError::Other(format!("Redis error: {e}"))))
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
        let plugin = self.try_get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("get_many");

        let bucket_handle = self.table.get(&bucket)?;
        let conn = bucket_handle.conn.clone();

        if keys.is_empty() {
            return Ok(Ok(vec![]));
        }

        let redis_keys: Vec<String> = keys.iter().map(|k| bucket_handle.prefixed_key(k)).collect();

        // Explicit MGET: the `mget` helper downgrades to `GET` for one key,
        // which decodes an absent key as an empty vec rather than one `None`.
        let values: Vec<Option<Vec<u8>>> = match conn
            .query_read(redis::cmd("MGET").arg(redis_keys.as_slice()))
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("Redis error getting keys: {}", e);
                return Ok(Err(StoreError::Other(format!("Redis error: {e}"))));
            }
        };

        let result = keys
            .into_iter()
            .zip(values)
            .map(|(key, value)| value.map(|v| (key, v)))
            .collect();

        Ok(Ok(result))
    }

    #[instrument(name = "wasi.keyvalue.set_many", skip(self, bucket, key_values))]
    async fn set_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let plugin = self.try_get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("set_many");

        let bucket_handle = self.table.get(&bucket)?;
        let conn = bucket_handle.conn.clone();

        if key_values.is_empty() {
            return Ok(Ok(()));
        }

        let pairs: Vec<(String, Vec<u8>)> = key_values
            .into_iter()
            .map(|(key, value)| (bucket_handle.prefixed_key(&key), value))
            .collect();

        match conn
            .query::<()>(redis::cmd("MSET").arg(pairs.as_slice()))
            .await
        {
            Ok(_) => Ok(Ok(())),
            Err(e) => {
                tracing::error!("Redis error setting keys: {}", e);
                Ok(Err(StoreError::Other(format!("Redis error: {e}"))))
            }
        }
    }

    #[instrument(name = "wasi.keyvalue.delete_many", skip(self, bucket, keys))]
    async fn delete_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        keys: Vec<String>,
    ) -> wasmtime::Result<Result<(), StoreError>> {
        let plugin = self.try_get_plugin::<RedisKeyValue>(PLUGIN_KEYVALUE_ID)?;

        plugin.record_operation("delete_many");

        let bucket_handle = self.table.get(&bucket)?;

        let values = futures::stream::FuturesOrdered::from_iter(keys.iter().map(|key| {
            let conn = bucket_handle.conn.clone();
            let redis_key = bucket_handle.prefixed_key(key);
            async move {
                match conn.query::<()>(redis::cmd("DEL").arg(redis_key)).await {
                    Ok(_) => Ok(()),
                    Err(e) => {
                        tracing::error!("Redis error deleting key: {}", e);
                        Err(StoreError::Other(format!("Redis error: {e}")))
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
        let linker = component_handle.linker()?;

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
    use super::*;

    fn config(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn connection_settings_default_to_the_v1_provider_values() {
        let settings = ConnectionSettings::from_config(&config(&[("url", "redis://h")])).unwrap();
        assert_eq!(settings, ConnectionSettings::default());
        assert_eq!(settings.reconnect_num_retries, 3);
        assert_eq!(settings.reconnect_max_delay_ms, 300);
        assert_eq!(settings.connection_timeout_ms, 3_000);
        assert_eq!(settings.response_timeout_ms, None);
    }

    #[test]
    fn connection_settings_read_the_v1_spelling_too() {
        let settings = ConnectionSettings::from_config(&config(&[
            ("BACKEND_RECONNECT_NUM_RETRIES", "5"),
            ("backend_reconnect_max_delay_ms", "1000"),
            ("Backend-Connection-Timeout-Ms", "250"),
            ("backend-response-timeout-ms", "2000"),
        ]))
        .unwrap();
        assert_eq!(
            settings,
            ConnectionSettings {
                reconnect_num_retries: 5,
                reconnect_max_delay_ms: 1_000,
                connection_timeout_ms: 250,
                response_timeout_ms: Some(2_000),
            }
        );
    }

    #[test]
    fn connection_settings_refuse_what_they_cannot_honor() {
        for (key, value, expect) in [
            (RECONNECT_NUM_RETRIES_KEY, "three", "whole number"),
            (RECONNECT_NUM_RETRIES_KEY, "-1", "whole number"),
            (RECONNECT_NUM_RETRIES_KEY, "1001", "between 0 and 1000"),
            (RECONNECT_MAX_DELAY_MS_KEY, "1.5", "whole number"),
            (RECONNECT_MAX_DELAY_MS_KEY, "600001", "between 0 and 600000"),
            (CONNECTION_TIMEOUT_MS_KEY, "0", "between 1 and 600000"),
            (CONNECTION_TIMEOUT_MS_KEY, "", "whole number"),
            (RESPONSE_TIMEOUT_MS_KEY, "0", "between 1 and 600000"),
            (RESPONSE_TIMEOUT_MS_KEY, "off", "whole number"),
        ] {
            let err = ConnectionSettings::from_config(&config(&[(key, value)]))
                .unwrap_err()
                .to_string();
            assert!(err.contains(key), "{key}={value:?}: {err}");
            assert!(err.contains(expect), "{key}={value:?}: {err}");
        }
    }

    #[test]
    fn connection_settings_refuse_one_key_set_twice_differently() {
        let err = ConnectionSettings::from_config(&config(&[
            ("BACKEND_CONNECTION_TIMEOUT_MS", "100"),
            ("backend-connection-timeout-ms", "200"),
        ]))
        .unwrap_err()
        .to_string();
        assert!(err.contains("twice"), "{err}");

        // The same value under two spellings is not a conflict.
        ConnectionSettings::from_config(&config(&[
            ("BACKEND_CONNECTION_TIMEOUT_MS", "100"),
            ("backend-connection-timeout-ms", "100"),
        ]))
        .unwrap();
    }

    #[test]
    fn the_pool_key_suffix_follows_the_parsed_values() {
        let a = ConnectionSettings::from_config(&config(&[("BACKEND_RESPONSE_TIMEOUT_MS", "500")]))
            .unwrap();
        let b =
            ConnectionSettings::from_config(&config(&[("backend-response-timeout-ms", "0500")]))
                .unwrap();
        let c = ConnectionSettings::from_config(&config(&[("backend-response-timeout-ms", "600")]))
            .unwrap();
        assert_eq!(a.pool_key_suffix(), b.pool_key_suffix());
        assert_ne!(a.pool_key_suffix(), c.pool_key_suffix());
        assert_ne!(
            a.pool_key_suffix(),
            ConnectionSettings::default().pool_key_suffix()
        );
    }

    /// The first connect is not put through the reconnect backoff: a refused
    /// connection is reported at once, however generous the redial schedule.
    #[tokio::test]
    async fn the_first_connect_fails_fast_when_nothing_listens() {
        // A port that was just free; nothing listens on it once this drops.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let client = redis::Client::open(format!("redis://127.0.0.1:{port}")).unwrap();
        // Ten redials with up to five seconds between them: a first connect put
        // through the redial schedule would take tens of seconds.
        let settings = ConnectionSettings::from_config(&config(&[
            (RECONNECT_NUM_RETRIES_KEY, "10"),
            (RECONNECT_MAX_DELAY_MS_KEY, "5000"),
        ]))
        .unwrap();
        let started = std::time::Instant::now();
        let err = Connection::connect(client, settings)
            .await
            .err()
            .expect("nothing listens on the port");
        let elapsed = started.elapsed();
        assert!(err.is_connection_refusal(), "{err}");
        assert!(
            elapsed < Duration::from_secs(1),
            "the first connect took {elapsed:?}"
        );
    }

    #[test]
    fn redial_pauses_never_exceed_the_configured_maximum() {
        for max in [0u64, 1, 50, 300, 10_000] {
            let settings = ConnectionSettings::from_config(&config(&[(
                RECONNECT_MAX_DELAY_MS_KEY,
                &max.to_string(),
            )]))
            .unwrap();
            for attempt in [0usize, 1, 2, 3, 10, 64, usize::MAX] {
                let ceiling = REDIAL_BASE_DELAY_MS
                    .saturating_mul(1 << u32::try_from(attempt).unwrap_or(u32::MAX).min(16))
                    .min(max);
                for _ in 0..50 {
                    let pause = settings.redial_delay(attempt);
                    assert!(
                        pause <= Duration::from_millis(max),
                        "attempt {attempt}, max {max}ms: {pause:?}"
                    );
                    assert!(
                        pause >= Duration::from_millis(ceiling / 2),
                        "attempt {attempt}, max {max}ms: {pause:?}"
                    );
                }
            }
        }
    }

    #[cfg(feature = "wasm_component_model_implements")]
    #[test]
    fn every_connection_setting_is_claimed_for_the_host() {
        for key in CONNECTION_SETTING_KEYS {
            assert!(
                crate::plugin::multiplex::KEYVALUE_HOST_OWNED_KEYS.contains(key),
                "{key} is not host-owned"
            );
        }
    }
}
