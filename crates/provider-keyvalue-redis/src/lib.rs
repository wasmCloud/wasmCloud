//! Redis implementation for wrpc:keyvalue.
//!
//! This implementation is multi-threaded and operations between different actors
//! use different connections and can run in parallel.
//! A single connection is shared by all instances of the same component id (public key),
//! so there may be some brief lock contention if several instances of the same component
//! are simultaneously attempting to communicate with redis. See documentation
//! on the [exec](#exec) function for more information.

use core::num::NonZeroU64;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{bail, Context as _};
use bytes::Bytes;
use redis::aio::ConnectionManager;
use redis::{Cmd, FromRedisValue};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, instrument, warn};
use unicase::UniCase;
use wasmcloud_provider_sdk::core::secrets::SecretValue;
use wasmcloud_provider_sdk::provider::WrpcClient;
use wasmcloud_provider_sdk::{
    get_connection, load_host_data, propagate_trace_for_ctx, run_provider, Context, HostData,
    LinkConfig, LinkDeleteInfo, Provider,
};
use wasmcloud_provider_sdk::{initialize_observability, serve_provider_exports};

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc:keyvalue/atomics@0.2.0-draft": generate,
            "wrpc:keyvalue/batch@0.2.0-draft": generate,
            "wrpc:keyvalue/store@0.2.0-draft": generate,
            "wrpc:keyvalue/watcher@0.2.0-draft": generate,
        }
    });
}
use bindings::exports::wrpc::keyvalue;
use wit_bindgen_wrpc::futures::StreamExt;

/// Default URL to use to connect to Redis
const DEFAULT_CONNECT_URL: &str = "redis://127.0.0.1:6379/";

/// Configuration key that will be used to search for Redis config
const CONFIG_REDIS_URL_KEY: &str = "URL";

type Result<T, E = keyvalue::store::Error> = core::result::Result<T, E>;

/// The default connection available for the redis client
///
/// This enum can be in different states which normally correspond to whether
/// the provider has started up (and the default connection has been created yet).
#[derive(Clone)]
pub enum DefaultConnection {
    /// Pre-supplied/available client configuration from config
    ClientConfig {
        config: HashMap<String, String>,
        secrets: Option<HashMap<String, SecretValue>>,
    },
    /// An already-initialized connection
    Conn(ConnectionManager),
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct WatchedKeyInfo {
    event_type: WatchEventType,
    target: String,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum WatchEventType {
    Set,
    Delete,
}
/// Represents a unique identifier for a link (target_id, link_name)
#[derive(Eq, Hash, PartialEq)]
struct LinkId {
    pub target_id: String,
    pub link_name: String,
}

/// Type for storing watch tasks associated with links
type WatchTaskMap = HashMap<LinkId, JoinHandle<()>>;

/// Redis `wrpc:keyvalue` provider implementation.
#[derive(Clone)]
pub struct KvRedisProvider {
    // store redis connections per source ID & link name
    sources: Arc<RwLock<HashMap<(String, String), ConnectionManager>>>,
    // default connection, which may be uninitialized
    default_connection: Arc<RwLock<DefaultConnection>>,
    // Stores information about watched keys for keyspace notifications
    // The outer HashMap uses the key as its key, and the HashSet contains
    // WatchedKeyInfo structs for each watcher of that key, allowing multiple
    // components to watch the same key for different event types.
    watched_keys: Arc<RwLock<HashMap<String, HashSet<WatchedKeyInfo>>>>,
    // Stores background tasks that handle keyspace notifications for each link
    watch_tasks: Arc<RwLock<WatchTaskMap>>,
}

pub async fn run() -> anyhow::Result<()> {
    KvRedisProvider::run().await
}

impl KvRedisProvider {
    pub fn name() -> &'static str {
        "keyvalue-redis-provider"
    }

    pub async fn run() -> anyhow::Result<()> {
        let host_data = load_host_data().context("failed to load host data")?;
        let flamegraph_path = host_data
            .config
            .get("FLAMEGRAPH_PATH")
            .map(String::from)
            .or_else(|| std::env::var("PROVIDER_KEYVALUE_REDIS_FLAMEGRAPH_PATH").ok());
        initialize_observability!(Self::name(), flamegraph_path);
        let provider = KvRedisProvider::from_host_data(host_data);
        let shutdown = run_provider(provider.clone(), KvRedisProvider::name())
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        let wrpc = connection
            .get_wrpc_client(connection.provider_key())
            .await?;
        serve_provider_exports(&wrpc, provider, shutdown, bindings::serve)
            .await
            .context("failed to serve provider exports")
    }

    #[must_use]
    pub fn from_config(config: HashMap<String, String>) -> Self {
        KvRedisProvider {
            sources: Arc::default(),
            default_connection: Arc::new(RwLock::new(DefaultConnection::ClientConfig {
                config,
                secrets: None,
            })),
            watched_keys: Arc::new(RwLock::new(HashMap::new())),
            watch_tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn from_host_data(host_data: &HostData) -> Self {
        KvRedisProvider {
            sources: Arc::default(),
            default_connection: Arc::new(RwLock::new(DefaultConnection::ClientConfig {
                config: host_data.config.clone(),
                secrets: Some(host_data.secrets.clone()),
            })),
            watched_keys: Arc::new(RwLock::new(HashMap::new())),
            watch_tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_default_connection(&self) -> anyhow::Result<ConnectionManager> {
        // NOTE: The read lock is only held for the duration of the `if let` block so we can acquire
        // the write lock to update the default connection if needed.
        if let DefaultConnection::Conn(conn) = &*self.default_connection.read().await {
            return Ok(conn.clone());
        }

        // Build the default conenction
        let mut default_conn = self.default_connection.write().await;
        match &mut *default_conn {
            DefaultConnection::Conn(conn) => Ok(conn.clone()),
            DefaultConnection::ClientConfig { config, secrets } => {
                let conn = redis::Client::open(retrieve_default_url(config, secrets))
                    .context("failed to construct default Redis client")?
                    .get_connection_manager()
                    .await
                    .context("failed to construct Redis connection manager")?;
                *default_conn = DefaultConnection::Conn(conn.clone());
                Ok(conn)
            }
        }
    }

    #[instrument(level = "debug", skip(self))]
    async fn invocation_conn(&self, context: Option<Context>) -> anyhow::Result<ConnectionManager> {
        let ctx = context.context("unexpectedly missing context")?;

        let Some(ref source_id) = ctx.component else {
            return self.get_default_connection().await.map_err(|err| {
                error!(error = ?err, "failed to get default connection for invocation");
                err
            });
        };

        let sources = self.sources.read().await;
        let Some(conn) = sources.get(&(source_id.into(), ctx.link_name().into())) else {
            error!(source_id, "no Redis connection found for component");
            bail!("No Redis connection found for component [{source_id}]. Please ensure the URL supplied in the link definition is a valid Redis URL")
        };

        Ok(conn.clone())
    }

    /// Execute Redis async command
    async fn exec_cmd<T: FromRedisValue>(
        &self,
        context: Option<Context>,
        cmd: &mut Cmd,
    ) -> Result<T, keyvalue::store::Error> {
        let mut conn = self
            .invocation_conn(context)
            .await
            .map_err(|err| keyvalue::store::Error::Other(format!("{err:#}")))?;
        match cmd.query_async(&mut conn).await {
            Ok(v) => Ok(v),
            Err(e) => {
                error!("failed to execute Redis command: {e}");
                Err(keyvalue::store::Error::Other(format!(
                    "failed to execute Redis command: {e}"
                )))
            }
        }
    }
}
#[instrument(level = "info", skip(wrpc))]
async fn invoke_on_set(wrpc: &WrpcClient, bucket: &str, key: &str, value: &Bytes) {
    let mut cx: async_nats::HeaderMap = async_nats::HeaderMap::new();
    for (k, v) in
        wasmcloud_provider_sdk::wasmcloud_tracing::context::TraceContextInjector::default_with_span(
        )
        .iter()
    {
        cx.insert(k.as_str(), v.as_str())
    }
    match bindings::wrpc::keyvalue::watcher::on_set(wrpc, Some(cx), bucket, key, value).await {
        Ok(_) => {
            debug!("successfully invoked on_set");
        }
        Err(err) => {
            error!(?err, "failed to invoke on_set");
        }
    }
    debug!("key set");
}
#[instrument(level = "info", skip(wrpc))]
async fn invoke_on_delete(wrpc: &WrpcClient, bucket: &str, key: &str) {
    let mut cx: async_nats::HeaderMap = async_nats::HeaderMap::new();
    for (k, v) in
        wasmcloud_provider_sdk::wasmcloud_tracing::context::TraceContextInjector::default_with_span(
        )
        .iter()
    {
        cx.insert(k.as_str(), v.as_str())
    }
    match bindings::wrpc::keyvalue::watcher::on_delete(wrpc, Some(cx), bucket, key).await {
        Ok(_) => {
            debug!("successfully invoked on_delete");
        }
        Err(err) => {
            error!(?err, "failed to invoke on_delete");
        }
    }
    debug!("key deleted");
}

impl keyvalue::store::Handler<Option<Context>> for KvRedisProvider {
    #[instrument(level = "debug", skip(self))]
    async fn delete(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<()>> {
        propagate_trace_for_ctx!(context);
        check_bucket_name(&bucket);
        Ok(self.exec_cmd(context, &mut Cmd::del(key)).await)
    }

    #[instrument(level = "debug", skip(self))]
    async fn exists(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<bool>> {
        propagate_trace_for_ctx!(context);
        check_bucket_name(&bucket);
        Ok(self.exec_cmd(context, &mut Cmd::exists(key)).await)
    }

    #[instrument(level = "debug", skip(self))]
    async fn get(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<Option<Bytes>>> {
        propagate_trace_for_ctx!(context);
        check_bucket_name(&bucket);
        match self
            .exec_cmd::<redis::Value>(context, &mut Cmd::get(key))
            .await
        {
            Ok(redis::Value::Nil) => Ok(Ok(None)),
            Ok(redis::Value::BulkString(buf)) => Ok(Ok(Some(buf.into()))),
            Ok(_) => Ok(Err(keyvalue::store::Error::Other(
                "invalid data type returned by Redis".into(),
            ))),
            Err(err) => Ok(Err(err)),
        }
    }

    #[instrument(level = "debug", skip(self))]
    async fn set(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
        value: Bytes,
    ) -> anyhow::Result<Result<()>> {
        propagate_trace_for_ctx!(context);
        check_bucket_name(&bucket);
        Ok(self
            .exec_cmd(context, &mut Cmd::set(key, value.to_vec()))
            .await)
    }

    #[instrument(level = "debug", skip(self))]
    async fn list_keys(
        &self,
        context: Option<Context>,
        bucket: String,
        cursor: Option<u64>,
    ) -> anyhow::Result<Result<keyvalue::store::KeyResponse>> {
        propagate_trace_for_ctx!(context);
        check_bucket_name(&bucket);
        match self
            .exec_cmd(
                context,
                redis::cmd("SCAN").cursor_arg(cursor.unwrap_or_default()),
            )
            .await
        {
            Ok((cursor, keys)) => Ok(Ok(keyvalue::store::KeyResponse {
                keys,
                cursor: NonZeroU64::new(cursor).map(Into::into),
            })),
            Err(err) => Ok(Err(err)),
        }
    }
}

impl keyvalue::atomics::Handler<Option<Context>> for KvRedisProvider {
    /// Increments a numeric value, returning the new value
    #[instrument(level = "debug", skip(self))]
    async fn increment(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
        delta: u64,
    ) -> anyhow::Result<Result<u64, keyvalue::store::Error>> {
        propagate_trace_for_ctx!(context);
        check_bucket_name(&bucket);
        Ok(self
            .exec_cmd::<u64>(context, &mut Cmd::incr(key, delta))
            .await)
    }
}

impl keyvalue::batch::Handler<Option<Context>> for KvRedisProvider {
    async fn get_many(
        &self,
        ctx: Option<Context>,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<Result<Vec<Option<(String, Bytes)>>>> {
        check_bucket_name(&bucket);
        let data = match self
            .exec_cmd::<Vec<Option<Bytes>>>(ctx, &mut Cmd::mget(&keys))
            .await
        {
            Ok(v) => v
                .into_iter()
                .zip(keys.into_iter())
                .map(|(val, key)| val.map(|b| (key, b)))
                .collect::<Vec<_>>(),
            Err(err) => {
                return Ok(Err(err));
            }
        };
        Ok(Ok(data))
    }

    async fn set_many(
        &self,
        ctx: Option<Context>,
        bucket: String,
        items: Vec<(String, Bytes)>,
    ) -> anyhow::Result<Result<()>> {
        check_bucket_name(&bucket);
        let items = items
            .into_iter()
            .map(|(name, buf)| (name, buf.to_vec()))
            .collect::<Vec<_>>();
        Ok(self.exec_cmd(ctx, &mut Cmd::mset(&items)).await)
    }

    async fn delete_many(
        &self,
        ctx: Option<Context>,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<Result<()>> {
        check_bucket_name(&bucket);
        Ok(self.exec_cmd(ctx, &mut Cmd::del(keys)).await)
    }
}

/// Handle provider control commands
impl Provider for KvRedisProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-component resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip(self, config))]
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            source_id,
            config,
            secrets,
            link_name,
            ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let url = secrets
            .keys()
            .find(|k| k.eq_ignore_ascii_case(CONFIG_REDIS_URL_KEY))
            .and_then(|url_key| config.get(url_key))
            .or_else(|| {
                warn!("redis connection URLs can be sensitive. Please consider using secrets to pass this value");
                config
                    .keys()
                    .find(|k| k.eq_ignore_ascii_case(CONFIG_REDIS_URL_KEY))
                    .and_then(|url_key| config.get(url_key))
            });

        let conn = if let Some(url) = url {
            match redis::Client::open(url.to_string()) {
                Ok(client) => match client.get_connection_manager().await {
                    Ok(conn) => {
                        info!(url, "established link");
                        conn
                    }
                    Err(err) => {
                        warn!(
                            url,
                            ?err,
                        "Could not create Redis connection manager for source [{source_id}], keyvalue operations will fail",
                    );
                        bail!("failed to create redis connection manager");
                    }
                },
                Err(err) => {
                    warn!(
                        ?err,
                        "Could not create Redis client for source [{source_id}], keyvalue operations will fail",
                    );
                    bail!("failed to create redis client");
                }
            }
        } else {
            self.get_default_connection().await.map_err(|err| {
                error!(error = ?err, "failed to get default connection for link");
                err
            })?
        };
        let mut sources = self.sources.write().await;
        sources.insert((source_id.to_string(), link_name.to_string()), conn);

        Ok(())
    }

    async fn receive_link_config_as_source(
        &self,
        LinkConfig {
            target_id,
            config,
            secrets,
            link_name,
            wit_metadata: (_, _, interfaces),
            ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let url = secrets
            .keys()
            .find(|k| k.eq_ignore_ascii_case(CONFIG_REDIS_URL_KEY))
            .and_then(|url_key| config.get(url_key))
            .or_else(|| {
                warn!("Redis connection URLs can be sensitive. Consider using secrets to pass this value.");
                config.keys()
                    .find(|k| k.eq_ignore_ascii_case(CONFIG_REDIS_URL_KEY))
                    .and_then(|url_key| config.get(url_key))
            })
            .map_or(DEFAULT_CONNECT_URL, |v| v);

        let client = match redis::Client::open(url.to_string()) {
            Ok(client) => {
                info!(url, "Established link at receive_link_config_as_source");
                client
            }
            Err(err) => {
                warn!(target_id = %target_id, err = ?err, "Failed to create Redis client");
                bail!("Failed to create Redis client");
            }
        };
        let mut conn = client.get_connection_manager().await.map_err(|e| {
            error!(err = ?e, "Failed to get async connection");
            anyhow::anyhow!("Failed to get async connection: {}", e)
        })?;

        let component_id: Arc<str> = target_id.into();
        let wrpc = get_connection()
            .get_wrpc_client(&component_id)
            .await
            .context("failed to construct wRPC client")?;
        if interfaces.contains(&"watcher".to_string()) {
            let config_response: Vec<String> = redis::cmd("CONFIG")
                .arg("GET")
                .arg("notify-keyspace-events")
                .query_async(&mut conn)
                .await
                .map_err(|e| {
                    error!(err = %e, "Failed to get keyspace notifications config");
                    anyhow::anyhow!("Failed to get keyspace notifications config: {}", e)
                })?;

            let current_config = config_response.get(1).ok_or_else(|| {
                error!("Unexpected response format from Redis CONFIG GET");
                anyhow::anyhow!("Unexpected response format from Redis CONFIG GET")
            })?;

            if !current_config.contains('K')
                || !current_config.contains('$')
                || !current_config.contains('g')
            {
                error!(
                    current_config = %current_config,
                    "Redis keyspace-notifications not properly configured"
                );
                return Err(anyhow::anyhow!(
                    "Redis keyspace-notifications not properly configured! \
                        Expected 'K$g' in settings, but got '{}'. \
                        Please run: CONFIG SET notify-keyspace-events K$g",
                    current_config
                ));
            }

            let wrpc = Arc::new(wrpc);
            let wrpc_for_task = wrpc.clone();

            let config_watch_entries = parse_watch_config(config, target_id);

            // Update watched keys
            let mut watched_keys = self.watched_keys.write().await;
            for (key, key_info_set) in config_watch_entries {
                watched_keys
                    .entry(key)
                    .or_insert_with(HashSet::new)
                    .extend(key_info_set);
            }

            let client_clone = client.clone();
            let self_clone = self.clone();
            let mut conn_clone = conn.clone();
            let task = tokio::spawn(async move {
                let mut pubsub = match client_clone.get_async_pubsub().await {
                    Ok(pubsub) => pubsub,
                    Err(e) => {
                        error!(err = %e, "Failed to get pubsub connection");
                        return;
                    }
                };
                let watched_keys = self_clone.watched_keys.read().await;
                for key in watched_keys.keys() {
                    let channel = format!("__keyspace@0__:{}", key);
                    let _ = pubsub
                        .psubscribe(&channel)
                        .await
                        .context("Failed to subscribe to SET/DEL events for key");
                }
                let stream = pubsub.on_message();
                tokio::pin!(stream);
                while let Some(msg) = stream.next().await {
                    let channel: String = msg.get_channel_name().to_string();
                    let event: String = match msg.get_payload() {
                        Ok(event) => event,
                        Err(e) => {
                            error!(err = %e, "Failed to get payload");
                            continue;
                        }
                    };
                    // The Channel is in the format __keyspace@0__:key
                    // While the payload is the event (ie set | del)
                    let mkey = match channel.split(':').next_back() {
                        Some(key) => key,
                        None => {
                            error!(channel = %channel, "Malformed Redis channel name: expected '__keyspace@0__:key' format");
                            continue;
                        }
                    };
                    // Check if the key is being watched by any component
                    let watched_keys = self_clone.watched_keys.read().await;
                    if let Some(key_info_set) = watched_keys.get(mkey) {
                        if event == "set" || event == "SET" {
                            // Perform a GET operation to retrieve the current value of the key since redis doesn't have a
                            // native way to get the value of the key from the notification
                            let value: wit_bindgen_wrpc::bytes::Bytes = match redis::cmd("GET")
                                .arg(mkey)
                                .query_async::<Option<Vec<u8>>>(&mut conn_clone)
                                .await
                            {
                                Ok(Some(v)) => v.into(),
                                Ok(None) => {
                                    debug!(key = %mkey, "Key not found or was deleted");
                                    continue;
                                }
                                Err(e) => {
                                    error!(key = %mkey, err = %e, "Failed to get value for key");
                                    continue;
                                }
                            };
                            for key_info in key_info_set {
                                if key_info.event_type == WatchEventType::Set {
                                    invoke_on_set(&wrpc_for_task, "0", mkey, &value).await;
                                }
                            }
                        } else if event == "del" || event == "DEL" {
                            for key_info in key_info_set {
                                if key_info.event_type == WatchEventType::Delete {
                                    invoke_on_delete(&wrpc_for_task, "0", mkey).await;
                                }
                            }
                        }
                    }
                }
            });
            let mut tasks = self.watch_tasks.write().await;
            tasks.insert(
                LinkId {
                    target_id: target_id.to_string(),
                    link_name: link_name.to_string(),
                },
                task,
            );
        }
        let mut sources = self.sources.write().await;
        sources.insert((target_id.to_string(), link_name.to_string()), conn);

        Ok(())
    }

    /// Handle notification that a link is dropped - close the connection
    #[instrument(level = "info", skip_all, fields(source_id = info.get_source_id()))]
    async fn delete_link_as_target(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_source_id();
        let mut aw = self.sources.write().await;
        // NOTE: ideally we should *not* get rid of all links for a given source here,
        // but delete_link actually does not tell us enough about the link to know whether
        // we're dealing with one link or the other.
        aw.retain(|(src_id, _link_name), _| src_id != component_id);
        debug!(component_id, "closing all redis connections for component");
        Ok(())
    }

    #[instrument(level = "info", skip_all, fields(target_id = info.get_target_id()))]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_target_id();
        let link_name = info.get_link_name();

        let mut sources = self.sources.write().await;
        sources.remove(&(component_id.to_string(), link_name.to_string()));

        let mut watch_tasks = self.watch_tasks.write().await;

        // If there's a watch task for this link, abort it and remove from map
        if let Some(task) = watch_tasks.remove(&LinkId {
            target_id: component_id.to_string(),
            link_name: link_name.to_string(),
        }) {
            task.abort();
            let _ = task.await;
        }

        // Clean up watched keys for this target
        let mut watched_keys = self.watched_keys.write().await;
        for key_watchers in watched_keys.values_mut() {
            key_watchers.retain(|key_info| key_info.target != component_id);
        }

        // Remove any empty watch sets
        watched_keys.retain(|_, watchers| !watchers.is_empty());

        debug!(
            component_id,
            link_name, "cleaned up redis connection and watch tasks for link"
        );
        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> anyhow::Result<()> {
        info!("shutting down");
        let mut aw = self.sources.write().await;
        // empty the component link data and stop all servers
        for (_, conn) in aw.drain() {
            drop(conn);
        }
        Ok(())
    }
}

/// Fetch the default URL to use for connecting to Redis from the configuration, defaulting
/// to `DEFAULT_CONNECT_URL` if no URL is found in the configuration.
fn retrieve_default_url(
    config: &HashMap<String, String>,
    secrets: &Option<HashMap<String, SecretValue>>,
) -> String {
    // Use connect URL provided by secrets first, if present
    if let Some(secrets) = secrets {
        if let Some(url) = secrets
            .keys()
            .find(|sk| sk.eq_ignore_ascii_case(CONFIG_REDIS_URL_KEY))
            .and_then(|k| secrets.get(k))
        {
            if let Some(s) = url.as_string() {
                debug!(
                    url = ?url, // NOTE: this is the SecretValue redacted output
                    "using Redis URL from secrets"
                );
                return s.into();
            } else {
                warn!("invalid secret value for URL (expected string, found bytes). Falling back to config");
            }
        }
    }

    // To aid in user experience, find the URL key in the config that matches "URL" in a case-insensitive manner
    let config_supplied_url = config
        .keys()
        .find(|k| k.eq_ignore_ascii_case(CONFIG_REDIS_URL_KEY))
        .and_then(|url_key| config.get(url_key));

    if let Some(url) = config_supplied_url {
        debug!(url, "using Redis URL from config");
        url.to_string()
    } else {
        debug!(DEFAULT_CONNECT_URL, "using default Redis URL");
        DEFAULT_CONNECT_URL.to_string()
    }
}
/// Parse watch configuration from the link configuration and return watch entries
///
/// Watch configuration is expected in the format "SET@key,DEL@key" where:
/// - SET: Watch for set operations on the specified key
/// - DEL: Watch for delete operations on the specified key
///
/// Returns a map of keys to sets of WatchedKeyInfo indicating which operations to watch for each key
#[instrument(level = "debug", skip(config))]
fn parse_watch_config(
    config: &HashMap<String, String>,
    target_id: &str,
) -> HashMap<String, HashSet<WatchedKeyInfo>> {
    let mut watched_keys = HashMap::new();

    // Convert config keys to case-insensitive map
    let config_map: HashMap<UniCase<&str>, &String> = config
        .iter()
        .map(|(k, v)| (UniCase::new(k.as_str()), v))
        .collect();

    // Look for watch configuration in the format "watch: SET@key,DEL@key"
    if let Some(watch_config) = config_map.get(&UniCase::new("watch")) {
        for watch_entry in watch_config.split(',') {
            let watch_entry = watch_entry.trim();
            if watch_entry.is_empty() {
                continue;
            }

            let parts: Vec<&str> = watch_entry.split('@').collect();
            if parts.len() != 2 {
                error!(watch_entry = %watch_entry, "Invalid watch entry format. Expected FORMAT@KEY");
                continue;
            }

            let operation = parts[0].trim().to_uppercase();
            let key_value = parts[1].trim();

            if key_value.contains(':') {
                error!(key = %key_value, "Invalid SET watch format. SET expects only KEY");
                continue;
            }
            if key_value.is_empty() {
                error!(watch_entry = %watch_entry, "Invalid watch entry: Missing key.");
                continue;
            }

            match operation.as_str() {
                "SET" => {
                    watched_keys
                        .entry(key_value.to_string())
                        .or_insert_with(HashSet::new)
                        .insert(WatchedKeyInfo {
                            event_type: WatchEventType::Set,
                            target: target_id.to_string(),
                        });
                }
                "DEL" => {
                    watched_keys
                        .entry(key_value.to_string())
                        .or_insert_with(HashSet::new)
                        .insert(WatchedKeyInfo {
                            event_type: WatchEventType::Delete,
                            target: target_id.to_string(),
                        });
                }
                _ => {
                    error!(operation = %operation, "Unsupported watch operation. Expected SET or DEL");
                }
            }
        }
    }

    watched_keys
}

/// Check for unsupported bucket names,
/// primarily warning on non-empty bucket names, since this provider does not yet properly support named buckets
fn check_bucket_name(bucket: &str) {
    if !bucket.is_empty() {
        warn!(bucket, "non-empty bucket names are not yet supported; ignoring non-empty bucket name (using a non-empty bucket name may become an error in the future).")
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashMap;

    use crate::retrieve_default_url;

    const PROPER_URL: &str = "redis://127.0.0.1:6379";

    #[test]
    fn can_deserialize_config_case_insensitive() {
        let lowercase_config = HashMap::from_iter([("url".to_string(), PROPER_URL.to_string())]);
        let uppercase_config = HashMap::from_iter([("URL".to_string(), PROPER_URL.to_string())]);
        let initial_caps_config = HashMap::from_iter([("Url".to_string(), PROPER_URL.to_string())]);

        assert_eq!(PROPER_URL, retrieve_default_url(&lowercase_config, &None));
        assert_eq!(PROPER_URL, retrieve_default_url(&uppercase_config, &None));
        assert_eq!(
            PROPER_URL,
            retrieve_default_url(&initial_caps_config, &None)
        );
    }

    #[test]
    fn test_parse_watch_config_valid_entries() {
        let mut config = HashMap::new();
        config.insert(
            "watch".to_string(),
            "SET@key1,DEL@key2,SET@key2".to_string(),
        );
        let target_id = "target_1";

        let result = parse_watch_config(&config, target_id);

        assert_eq!(result.len(), 2);
        assert!(result.contains_key("key1"));
        assert!(result.contains_key("key2"));

        assert!(result["key1"].contains(&WatchedKeyInfo {
            event_type: WatchEventType::Set,
            target: target_id.to_string()
        }));
        assert!(result["key2"].contains(&WatchedKeyInfo {
            event_type: WatchEventType::Delete,
            target: target_id.to_string()
        }));
        assert!(result["key2"].contains(&WatchedKeyInfo {
            event_type: WatchEventType::Set,
            target: target_id.to_string()
        }));
    }

    #[test]
    fn test_parse_watch_config_invalid_entries() {
        let mut config = HashMap::new();
        config.insert(
            "watch".to_string(),
            "INVALID@key1,SET@key2,DEL@key3,SET@key4:extra".to_string(),
        );
        let target_id = "target_2";

        let result = parse_watch_config(&config, target_id);

        assert_eq!(result.len(), 2);
        assert!(result.contains_key("key2"));
        assert!(result.contains_key("key3"));

        assert!(result["key2"].contains(&WatchedKeyInfo {
            event_type: WatchEventType::Set,
            target: target_id.to_string()
        }));
        assert!(result["key3"].contains(&WatchedKeyInfo {
            event_type: WatchEventType::Delete,
            target: target_id.to_string()
        }));
    }

    #[test]
    fn test_parse_watch_config_empty_or_malformed() {
        let mut config = HashMap::new();
        config.insert("watch".to_string(), "SET@,DEL@ , @key5".to_string());
        let target_id = "target_3";

        let result = parse_watch_config(&config, target_id);

        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_watch_config_case_insensitivity() {
        let mut config = HashMap::new();
        config.insert("WATCH".to_string(), "set@key1,del@key2".to_string());
        let target_id = "target_4";

        let result = parse_watch_config(&config, target_id);

        assert_eq!(result.len(), 2);
        assert!(result.contains_key("key1"));
        assert!(result.contains_key("key2"));

        assert!(result["key1"].contains(&WatchedKeyInfo {
            event_type: WatchEventType::Set,
            target: target_id.to_string()
        }));
        assert!(result["key2"].contains(&WatchedKeyInfo {
            event_type: WatchEventType::Delete,
            target: target_id.to_string()
        }));
    }

    #[test]
    fn test_parse_watch_config_no_watch_key() {
        let config = HashMap::new();
        let target_id = "target_5";

        let result = parse_watch_config(&config, target_id);

        assert!(result.is_empty());
    }
}
