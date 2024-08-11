//! Redis implementation for wrpc:keyvalue.
//!
//! This implementation is multi-threaded and operations between different actors
//! use different connections and can run in parallel.
//! A single connection is shared by all instances of the same component id (public key),
//! so there may be some brief lock contention if several instances of the same component
//! are simultaneously attempting to communicate with redis. See documentation
//! on the [exec](#exec) function for more information.

use core::num::NonZeroU64;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _};
use bytes::Bytes;
use redis::aio::ConnectionManager;
use redis::{Cmd, FromRedisValue};
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument, warn};
use wasmcloud_provider_sdk::{
    get_connection, load_host_data, propagate_trace_for_ctx, run_provider, Context, LinkConfig,
    Provider,
};
use wasmcloud_provider_sdk::{initialize_observability, serve_provider_exports};

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc:keyvalue/atomics@0.2.0-draft": generate,
            "wrpc:keyvalue/batch@0.2.0-draft": generate,
            "wrpc:keyvalue/store@0.2.0-draft": generate,
            // TODO: Implement the watch interface and add its binding
            // "wrpc:keyvalue/watch@0.2.0-draft": generate,
        }
    });
}
use bindings::exports::wrpc::keyvalue;

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

/// Redis `wrpc:keyvalue` provider implementation.
#[derive(Clone)]
pub struct KvRedisProvider {
    // store redis connections per source ID & link name
    sources: Arc<RwLock<HashMap<(String, String), ConnectionManager>>>,
    // default connection, which may be uninitialized
    default_connection: Arc<RwLock<DefaultConnection>>,
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
        serve_provider_exports(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
            shutdown,
            bindings::serve,
        )
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
        Ok(self.exec_cmd(ctx, &mut Cmd::mget(&keys)).await)
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
        Ok(self.exec_cmd(ctx, &mut Cmd::del(&keys)).await)
    }
}

// TODO: 
// Implementation of the watch on-set and on-delete interface.
// Every time there is a change on the bucket, the provider needs to
// inform the component interested in on-set or on-delete events:
// If a bucket is added, invoke on_set.
// If a bucket is deleted, invoke on_delete.
// provider (src) --> component (dst)
/*
impl keyvalue::watch::Handler<Option<Context>> for KvRedisProvider {
    // map to store watch server (and its link parameters) for each linked component
    // Start a server instance that calls the given component whenever there is a change in the bucket
    
    async fn on_set(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
        value: list<u8>,
    ) -> anyhow::Result<Result<u64, keyvalue::store::Error>> {
        propagate_trace_for_ctx!(context);
        check_bucket_name(&bucket);
        Ok(self
            // send the on-set notification to the given component           
            // 
            .await)
    }        

    async fn on_delete(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<u64, keyvalue::store::Error>> {
        propagate_trace_for_ctx!(context);
        check_bucket_name(&bucket);
        Ok(self
            // send the on-delete notification to the given component           
            // 
            .await)
    } 
}
*/

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

    /// Handle notification that a link is dropped - close the connection
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, source_id: &str) -> anyhow::Result<()> {
        let mut aw = self.sources.write().await;
        // NOTE: ideally we should *not* get rid of all links for a given source here,
        // but delete_link actually does not tell us enough about the link to know whether
        // we're dealing with one link or the other.
        aw.retain(|(src_id, _link_name), _| src_id != source_id);
        debug!(
            component_id = source_id,
            "closing all redis connections for component"
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
