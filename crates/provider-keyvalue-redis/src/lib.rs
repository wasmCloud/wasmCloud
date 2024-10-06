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
use wasmcloud_provider_sdk::provider::WrpcClient;
use wasmcloud_provider_sdk::{
    get_connection, load_host_data, propagate_trace_for_ctx, run_provider, Context, LinkConfig,
    LinkDeleteInfo, Provider,
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

#[derive(Clone)]
pub enum DefaultConnection {
    ClientConfig(HashMap<String, String>),
    Conn(ConnectionManager),
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct WatchedKeyInfo {
    event_type: WatchEventType,
    value: Option<String>,
    target: String,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum WatchEventType {
    Set,
    Delete,
}

/// Represents a unique identifier for a link (target_id, link_name)
type LinkId = (String, String);

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
        let provider = KvRedisProvider::new(host_data.config.clone());
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
    pub fn new(initial_config: HashMap<String, String>) -> Self {
        KvRedisProvider {
            sources: Arc::default(),
            default_connection: Arc::new(RwLock::new(DefaultConnection::ClientConfig(
                initial_config,
            ))),
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

        let mut default_conn = self.default_connection.write().await;
        match &mut *default_conn {
            DefaultConnection::Conn(conn) => Ok(conn.clone()),
            DefaultConnection::ClientConfig(cfg) => {
                let conn = redis::Client::open(retrieve_default_url(cfg))
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
            info!("Successfully invoked on_set for key '{key}' from bucket '{bucket}'");
        }
        Err(err) => {
            error!("Failed to invoke on_set for bucket '{bucket}' and key '{key}': {err}");

            if let Some(source) = err.source() {
                error!("Caused by: {}", source);
            }

            let mut error_chain = String::new();
            let mut current_error = err.source();
            while let Some(error) = current_error {
                error_chain.push_str(&format!("\n  → {}", error));
                current_error = error.source();
            }
            if !error_chain.is_empty() {
                error!("Error chain:{}", error_chain);
            }

            #[cfg(debug_assertions)]
            {
                error!(
                    "Debug backtrace: {:?}",
                    std::backtrace::Backtrace::capture()
                );
            }
        }
    }

    info!("Key set: {}", key);
}

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
            info!("Successfully invoked on_delete key '{key}' from bucket '{bucket}'");
        }
        Err(err) => {
            error!("Failed to invoke on_delete for bucket '{bucket}' and key '{key}': {err}");

            if let Some(source) = err.source() {
                error!("Caused by: {}", source);
            }

            let mut error_chain = String::new();
            let mut current_error = err.source();
            while let Some(error) = current_error {
                error_chain.push_str(&format!("\n  → {}", error));
                current_error = error.source();
            }
            if !error_chain.is_empty() {
                error!("Error chain:{}", error_chain);
            }

            #[cfg(debug_assertions)]
            {
                error!(
                    "Debug backtrace: {:?}",
                    std::backtrace::Backtrace::capture()
                );
            }
        }
    }
    info!("Key deleted: {}", key);
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
            Ok(redis::Value::Data(buf)) => Ok(Ok(Some(buf.into()))),
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

impl keyvalue::watcher::Handler<Option<Context>> for KvRedisProvider {
    async fn on_set(
        &self,
        cx: Option<Context>,
        bucket: String,
        key: String,
        value: wit_bindgen_wrpc::bytes::Bytes,
    ) -> wit_bindgen_wrpc::anyhow::Result<()> {
        check_bucket_name(&bucket);
        let Some(target) = cx.as_ref().and_then(|c| c.component.clone()) else {
            error!("received on_set invocation without a source component");
            bail!("received on_set invocation without a source component")
        };
        let value_str = String::from_utf8(value.to_vec()).unwrap_or_default();
        // Add the key to the watched keys
        let mut watched_keys = self.watched_keys.write().await;
        watched_keys
            .entry(key.clone())
            .or_default()
            .insert(WatchedKeyInfo {
                event_type: WatchEventType::Set,
                value: Some(value_str.clone()),
                target,
            });
        info!("Watching key : '{key}' for Set Operation => value : '{value_str}'");
        Ok(())
    }

    async fn on_delete(
        &self,
        cx: Option<Context>,
        bucket: String,
        key: String,
    ) -> wit_bindgen_wrpc::anyhow::Result<()> {
        check_bucket_name(&bucket);
        let Some(target) = cx.as_ref().and_then(|c| c.component.clone()) else {
            error!("received on_delete invocation without a source component");
            bail!("received on_delete invocation without a source component")
        };

        // Add the key to the watched keys
        let mut watched_keys = self.watched_keys.write().await;
        watched_keys
            .entry(key.clone())
            .or_default()
            .insert(WatchedKeyInfo {
                event_type: WatchEventType::Delete,
                value: None,
                target,
            });
        info!("Watching key : '{key}' for deletion");
        Ok(())
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
                warn!(?err, "Failed to create Redis client for target [{target_id}]. Key-value operations will fail.");
                bail!("Failed to create Redis client");
            }
        };

        let mut conn = client.get_connection_manager().await.map_err(|e| {
            error!("Failed to get async connection: {e}");
            anyhow::anyhow!("Failed to get async connection: {}", e)
        })?;

        let component_id: Arc<str> = target_id.into();
        let wrpc = get_connection()
            .get_wrpc_client(&component_id)
            .await
            .context("failed to construct wRPC client")?;
        if interfaces.contains(&"watcher".to_string()) {
            let wrpc = Arc::new(wrpc);
            let wrpc_for_task = wrpc.clone();
            // Set keyspace notifications
            redis::cmd("CONFIG")
                .arg("SET")
                .arg("notify-keyspace-events")
                .arg("KEA")
                .query_async::<_, ()>(&mut conn)
                .await
                .map_err(|e| {
                    error!("Failed to set keyspace notifications: {e}");
                    anyhow::anyhow!("Failed to set keyspace notifications: {}", e)
                })?;

            // Start watching for keyspace events
            let client_clone = client.clone();
            let self_clone = self.clone();
            let mut conn_clone = conn.clone();
            let task = tokio::spawn(async move {
                let mut pubsub = match client_clone.get_async_pubsub().await {
                    Ok(pubsub) => pubsub,
                    Err(e) => {
                        error!("Failed to get pubsub connection: {}", e);
                        return;
                    }
                };
                pubsub.psubscribe("__keyevent@0__:set").await.unwrap();
                pubsub.psubscribe("__keyevent@0__:del").await.unwrap();
                let stream = pubsub.on_message();
                tokio::pin!(stream);
                while let Some(msg) = stream.next().await {
                    let channel: String = msg.get_channel_name().to_string();
                    let mkey: String = match msg.get_payload() {
                        Ok(mkey) => mkey,
                        Err(e) => {
                            error!("Failed to get payload: {}", e);
                            continue;
                        }
                    };
                    // The Channel name is in the format __keyevent@0__:set or __keyevent@0__:del
                    // While the payload is the key that was set or deleted
                    let event = channel.split(':').last().unwrap();
                    // Check if the key is being watched by any component
                    let watched_keys = self_clone.watched_keys.read().await;
                    if let Some(key_info_set) = watched_keys.get(mkey.as_str()) {
                        if event == "set" {
                            // Perform a GET operation to retrieve the current value of the key since redis doesn't have a
                            // native way to get the value of the key from the notification
                            let value: wit_bindgen_wrpc::bytes::Bytes = match redis::cmd("GET")
                                .arg(&mkey)
                                .query_async::<_, Option<Vec<u8>>>(&mut conn_clone)
                                .await
                            {
                                Ok(Some(v)) => v.into(),
                                Ok(None) => {
                                    info!("Key '{mkey}' not found or was deleted");
                                    continue;
                                }
                                Err(e) => {
                                    error!("Failed to get value for key {}: {}", mkey, e);
                                    continue;
                                }
                            };
                            info!(
                                "Value retrieved from kv store for key {} is {:?}",
                                mkey, value
                            );
                            for key_info in key_info_set {
                                if key_info.event_type == WatchEventType::Set {
                                    if key_info.value.is_none() {
                                        error!("Value for key {} is not set ", mkey);
                                        continue;
                                    }
                                    if let Ok(str_value) = String::from_utf8(value.to_vec()) {
                                        if key_info.value == Some(str_value) {
                                            invoke_on_set(&wrpc_for_task, "0", &mkey, &value).await;
                                        }
                                    } else {
                                        error!("Value for key {} is not valid UTF-8", mkey);
                                        continue;
                                    }
                                }
                            }
                        } else if event == "del" {
                            for key_info in key_info_set {
                                if key_info.event_type == WatchEventType::Delete {
                                    invoke_on_delete(&wrpc_for_task, "0", &mkey).await;
                                }
                            }
                        }
                    }
                }
            });
            let mut tasks = self.watch_tasks.write().await;
            tasks.insert((target_id.to_string(), link_name.to_string()), task);
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
        let mut aw = self.sources.write().await;
        aw.retain(|(src_id, _link_name), _| src_id != component_id);
        // we remove the watch task associated with the link
        // we should make sure all the tasks are stopped belonging to the link, so we should loop through it
        let mut watch_tasks = self.watch_tasks.write().await;
        let mut keys_to_remove = Vec::new();
        for (link_id, task) in watch_tasks.iter() {
            if link_id.0 == component_id {
                // NOTE: There's probably a much cleaner way to abort this task.
                task.abort();
                keys_to_remove.push(link_id.clone());
            }
        }
        for key in keys_to_remove {
            watch_tasks.remove(&key);
        }
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
pub fn retrieve_default_url(config: &HashMap<String, String>) -> String {
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

/// Check for unsupported bucket names,
/// primarily warning on non-empty bucket names, since this provider does not yet properly support named buckets
fn check_bucket_name(bucket: &str) {
    if !bucket.is_empty() {
        warn!(bucket, "non-empty bucket names are not yet supported; ignoring non-empty bucket name (using a non-empty bucket name may become an error in the future).")
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use crate::retrieve_default_url;

    const PROPER_URL: &str = "redis://127.0.0.1:6379";

    #[test]
    fn can_deserialize_config_case_insensitive() {
        let lowercase_config = HashMap::from_iter([("url".to_string(), PROPER_URL.to_string())]);
        let uppercase_config = HashMap::from_iter([("URL".to_string(), PROPER_URL.to_string())]);
        let initial_caps_config = HashMap::from_iter([("Url".to_string(), PROPER_URL.to_string())]);

        assert_eq!(PROPER_URL, retrieve_default_url(&lowercase_config));
        assert_eq!(PROPER_URL, retrieve_default_url(&uppercase_config));
        assert_eq!(PROPER_URL, retrieve_default_url(&initial_caps_config));
    }
}
