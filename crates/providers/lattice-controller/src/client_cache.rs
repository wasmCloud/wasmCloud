use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::sync::RwLock;
use tokio::time::{interval_at, Duration, Instant};
use tracing::{debug, trace};
use wascap::prelude::KeyPair;
use wasmcloud_control_interface::Client;

use crate::ConnectionConfig;

#[derive(Clone)]
pub(crate) struct ClientCache {
    meta: Arc<RwLock<HashMap<String, ClientMetadata>>>,
    clients: Arc<RwLock<HashMap<String, Client>>>,
}

#[derive(Debug, Clone)]
struct ClientMetadata {
    config: ConnectionConfig,
    last_accessed: Instant,
}

impl ClientMetadata {
    fn touch(&mut self) {
        self.last_accessed = Instant::now();
    }
}

impl ClientCache {
    /// Creates a new client cache. Configures and starts the cache item expiration timer
    pub(crate) async fn new(expire_in_seconds: u64) -> Self {
        let meta = RwLock::new(HashMap::new());
        let clients = RwLock::new(HashMap::new());

        let m = Arc::new(meta);
        let c = Arc::new(clients);

        let period = Duration::from_secs(expire_in_seconds);
        let start = Instant::now() + period;
        let mut task = interval_at(start, period);

        let cc = ClientCache {
            meta: m.clone(),
            clients: c.clone(),
        };

        tokio::spawn(async move {
            loop {
                task.tick().await;
                evacuate_cache(m.clone(), c.clone(), period).await;
            }
        });

        cc
    }

    // At the moment the only thing that uses this function is the test
    #[allow(dead_code)]
    pub(crate) async fn remove_config(&self, lattice_id: &str) {
        let mut m = self.meta.write().await;

        m.remove(lattice_id);
    }

    /// Stores a connection configuration corresponding to a given lattice. No side effects, does _not_
    /// create or establish a NATS connection
    pub(crate) async fn put_config(&self, lattice_id: &str, config: ConnectionConfig) {
        let mut m = self.meta.write().await;

        m.insert(
            lattice_id.to_string(),
            ClientMetadata {
                config,
                last_accessed: Instant::now(),
            },
        );
    }

    /// Retrieves a client from the cache. If one is already active, this will be returned. If not,
    /// one will be created from the stored connection configuration. If there is no active client
    /// and no suitable configuration, this function returns an error and will _not_ resort to
    /// fallback credentials
    pub(crate) async fn get_client(&self, lattice_id: &str) -> Result<Client> {
        let c = {
            // Don't hold the read lock for the whole func
            let lock = self.clients.read().await;
            lock.get(lattice_id).cloned()
        };
        if let Some(c) = c {
            self.record_access(lattice_id).await;
            Ok(c)
        } else {
            let meta = {
                // Dispose of lock as soon as we get what we need
                let lock = self.meta.read().await;
                lock.get(lattice_id).cloned()
            };
            if let Some(cfg) = meta {
                let client = create_client(&cfg.config).await?;
                self.store_client(lattice_id, client.clone()).await;
                Ok(client)
            } else {
                bail!("no client configuration for lattice [{lattice_id}] stored");
            }
        }
    }

    async fn store_client(&self, lattice_id: &str, client: Client) {
        let mut conns = self.clients.write().await;
        conns.insert(lattice_id.to_string(), client);
    }

    async fn record_access(&self, lattice_id: &str) {
        let mut meta = self.meta.write().await;
        meta.entry(lattice_id.to_string()).and_modify(|e| e.touch());
    }
}

/// Create and connect a [`wasmcloud_control_interface::Client`] interface client, given a [`ConnectionConfig`]
async fn create_client(config: &ConnectionConfig) -> Result<wasmcloud_control_interface::Client> {
    let timeout = Duration::from_millis(config.timeout_ms);
    let auction_timeout = Duration::from_millis(config.auction_timeout_ms);
    let lattice = config.lattice.clone();
    let conn = connect(config).await?;

    Ok(wasmcloud_control_interface::ClientBuilder::new(conn)
        .lattice(lattice)
        .timeout(timeout)
        .auction_timeout(auction_timeout)
        .build())
}

/// Create a new nats connection
async fn connect(cfg: &ConnectionConfig) -> Result<async_nats::Client> {
    let cfg = cfg.clone();
    let opts = match (cfg.auth_jwt, cfg.auth_seed) {
        (Some(jwt), Some(seed)) => {
            let key_pair = std::sync::Arc::new(KeyPair::from_seed(&seed).context("key init: {e}")?);
            async_nats::ConnectOptions::with_jwt(jwt, move |nonce| {
                let key_pair = key_pair.clone();
                async move { key_pair.sign(&nonce).map_err(async_nats::AuthError::new) }
            })
        }
        (None, None) => async_nats::ConnectOptions::default(),
        _ => {
            bail!("must provide both jwt and seed for jwt authentication");
        }
    };
    if cfg.cluster_uris.is_empty() {
        bail!("No NATS URIs supplied");
    }

    let url = cfg.cluster_uris.first().unwrap();

    let conn = opts
        .event_callback(|event| async move {
            // lattice prefix/ID will already be on the span from earlier calls
            match event {
                async_nats::Event::Disconnected => debug!("NATS client disconnected"),
                async_nats::Event::Connected => debug!("NATS client reconnected"),
                async_nats::Event::ClientError(err) => {
                    debug!("NATS client error occurred: {err}")
                }
                other => debug!("NATS client other event occurred: {other}"),
            }
        })
        .connect(url)
        .await
        .with_context(|| format!("Nats connection to {url}"))?;

    Ok(conn)
}

/// Discovers a list of expired (access time within grace period) connections
/// and then removes them from the cache.
async fn evacuate_cache(
    m: Arc<RwLock<HashMap<String, ClientMetadata>>>,
    c: Arc<RwLock<HashMap<String, Client>>>,
    period: Duration,
) {
    let expired_keys: Vec<String> = {
        let meta = m.read().await;

        meta.iter()
            .filter(|(_k, v)| v.last_accessed.elapsed() > period)
            .map(|(k, _v)| k.to_string())
            .collect()
    };

    if !expired_keys.is_empty() {
        trace!(
            "Removing NATS clients from cache: {}",
            expired_keys.join(",")
        );
    }

    let mut conns = c.write().await;
    conns.retain(|k, _v| !expired_keys.contains(k));
}

/// The test suite below requires an anonymous localhost NATS
///
/// You can run one locally using `docker`:
///
/// ```console
/// docker run --rm -p 4222:4222 nats -js
/// ```
#[cfg(test)]
mod test {
    use std::time::Duration;

    use crate::ConnectionConfig;

    use super::ClientCache;

    #[tokio::test]
    async fn test_cache_evacuation() {
        let cache = ClientCache::new(2).await;
        cache.put_config("test", ConnectionConfig::default()).await;

        let _client = cache.get_client("test").await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;

        // client should no longer be in the cache because it hasn't been utilized.
        // this will reconstitute the client
        let res = cache.get_client("test").await;
        assert!(res.is_ok());

        tokio::time::sleep(Duration::from_secs(5)).await;
        cache.remove_config("test").await;
        // Now that there's no config, attempting to get client will be a cache
        // miss and there won't be config to create a new connection.

        let res = cache.get_client("test").await;
        assert!(res.is_err());
    }
}
