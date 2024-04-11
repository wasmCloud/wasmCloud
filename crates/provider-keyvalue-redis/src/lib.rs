//! Redis implementation for wasmcloud:keyvalue.
//!
//! This implementation is multi-threaded and operations between different actors
//! use different connections and can run in parallel.
//! A single connection is shared by all instances of the same actor id (public key),
//! so there may be some brief lock contention if several instances of the same actor
//! are simultaneously attempting to communicate with redis. See documentation
//! on the [exec](#exec) function for more information.
//!
//! Note that this provider uses many *re-exported* dependencies of `wasmcloud_provider_wit_bindgen`
//! in order to reduce required dependencies on this binary itself. Using `serde` as a re-exported dependency
//! requires changing the crate location of `serde` with the `#[serde(crate = "...")]` annotation.
//!
//!
use core::num::NonZeroU64;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use redis::aio::ConnectionManager;
use redis::{Cmd, FromRedisValue};
use tokio::sync::RwLock;
use tracing::{error, info, instrument, warn};

use wasmcloud_provider_sdk::core::HostData;
use wasmcloud_provider_sdk::{
    get_connection, load_host_data, run_provider, Context, LinkConfig, Provider,
};

use exports::wrpc::keyvalue;

wit_bindgen_wrpc::generate!();

/// Default URL to use to connect to Redis
const DEFAULT_CONNECT_URL: &str = "redis://127.0.0.1:6379/";

/// Configuration key that will be used to search for Redis config
const CONFIG_REDIS_URL_KEY: &str = "URL";

type Result<T, E = keyvalue::store::Error> = core::result::Result<T, E>;

#[derive(Clone)]
pub enum DefaultConnection {
    Client(redis::Client),
    Conn(ConnectionManager),
}

/// Redis `wrpc:keyvalue` provider implementation.
#[derive(Clone)]
pub struct KvRedisProvider {
    // store redis connections per source ID
    sources: Arc<RwLock<HashMap<String, ConnectionManager>>>,
    // default connection, which may be uninitialized
    default_connection: Arc<RwLock<DefaultConnection>>,
}

pub async fn run() -> anyhow::Result<()> {
    KvRedisProvider::run().await
}

impl KvRedisProvider {
    pub async fn run() -> anyhow::Result<()> {
        let HostData { config, .. } = load_host_data().context("failed to load host data")?;
        let client = redis::Client::open(retrieve_default_url(config))
            .context("failed to construct default Redis client")?;
        let default_connection = if let Ok(conn) = client.get_connection_manager().await {
            DefaultConnection::Conn(conn)
        } else {
            DefaultConnection::Client(client)
        };
        let provider = KvRedisProvider::new(default_connection);
        let shutdown = run_provider(provider.clone(), "keyvalue-redis-provider")
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        serve(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
            shutdown,
        )
        .await
    }

    #[must_use]
    pub fn new(default_connection: DefaultConnection) -> Self {
        KvRedisProvider {
            sources: Arc::default(),
            default_connection: Arc::new(RwLock::new(default_connection)),
        }
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_default_connection(&self) -> anyhow::Result<ConnectionManager> {
        if let DefaultConnection::Conn(conn) = &*self.default_connection.read().await {
            Ok(conn.clone())
        } else {
            let mut default_conn = self.default_connection.write().await;
            match &mut *default_conn {
                DefaultConnection::Conn(conn) => Ok(conn.clone()),
                DefaultConnection::Client(client) => {
                    let conn = client
                        .get_connection_manager()
                        .await
                        .context("failed to construct Redis connection manager")?;
                    *default_conn = DefaultConnection::Conn(conn.clone());
                    Ok(conn)
                }
            }
        }
    }

    #[instrument(level = "debug", skip(self))]
    async fn invocation_conn(&self, context: Option<Context>) -> anyhow::Result<ConnectionManager> {
        if let Some(ref source_id) = context.and_then(|Context { component, .. }| component) {
            let sources = self.sources.read().await;
            let Some(conn) = sources.get(source_id) else {
                error!("No Redis connection found for actor [{source_id}]. Please ensure the URL supplied in the link definition is a valid Redis URL");
                bail!("No Redis connection found for actor [{source_id}]. Please ensure the URL supplied in the link definition is a valid Redis URL")
            };
            Ok(conn.clone())
        } else {
            self.get_default_connection().await.map_err(|err| {
                error!(?err, "failed to get default connection for invocation");
                err
            })
        }
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

impl keyvalue::store::Handler<Option<Context>> for KvRedisProvider {
    #[instrument(level = "debug", skip(self))]
    async fn delete(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<()>> {
        // TODO: Use bucket
        _ = bucket;
        Ok(self.exec_cmd(context, &mut Cmd::del(key)).await)
    }

    #[instrument(level = "debug", skip(self))]
    async fn exists(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<bool>> {
        // TODO: Use bucket
        _ = bucket;
        Ok(self.exec_cmd(context, &mut Cmd::exists(key)).await)
    }

    #[instrument(level = "debug", skip(self))]
    async fn get(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<Option<Vec<u8>>>> {
        // TODO: Use bucket
        _ = bucket;
        match self
            .exec_cmd::<redis::Value>(context, &mut Cmd::get(key))
            .await
        {
            Ok(redis::Value::Nil) => Ok(Ok(None)),
            Ok(redis::Value::Data(buf)) => Ok(Ok(Some(buf))),
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
        value: Vec<u8>,
    ) -> anyhow::Result<Result<()>> {
        // TODO: Use bucket
        _ = bucket;
        Ok(self.exec_cmd(context, &mut Cmd::set(key, value)).await)
    }

    #[instrument(level = "debug", skip(self))]
    async fn list_keys(
        &self,
        context: Option<Context>,
        bucket: String,
        cursor: Option<u64>,
    ) -> anyhow::Result<Result<keyvalue::store::KeyResponse>> {
        // TODO: Use bucket
        _ = bucket;
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
        // TODO: Use bucket
        _ = bucket;
        Ok(self
            .exec_cmd::<u64>(context, &mut Cmd::incr(key, delta))
            .await)
    }
}

/// Handle provider control commands
impl Provider for KvRedisProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip(self, config))]
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            source_id, config, ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let conn = if let Some(url) = config.get(CONFIG_REDIS_URL_KEY) {
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
                        return Err(anyhow!("failed to create redis connection manager"));
                    }
                },
                Err(err) => {
                    warn!(
                        ?err,
                        "Could not create Redis client for source [{source_id}], keyvalue operations will fail",
                    );
                    return Err(anyhow!("failed to create redis client"));
                }
            }
        } else {
            self.get_default_connection().await.map_err(|err| {
                error!(?err, "failed to get default connection for link");
                err
            })?
        };
        let mut sources = self.sources.write().await;
        sources.insert(source_id.to_string(), conn);

        Ok(())
    }

    /// Handle notification that a link is dropped - close the connection
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, source_id: &str) -> anyhow::Result<()> {
        let mut aw = self.sources.write().await;
        if let Some(conn) = aw.remove(source_id) {
            info!("redis closing connection for actor {}", source_id);
            drop(conn);
        }
        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> anyhow::Result<()> {
        let mut aw = self.sources.write().await;
        // empty the actor link data and stop all servers
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
        info!(url, "Using Redis URL from config");
        url.to_string()
    } else {
        info!(DEFAULT_CONNECT_URL, "Using default Redis URL");
        DEFAULT_CONNECT_URL.to_string()
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
