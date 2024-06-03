//! NATS implementation for wrpc:keyvalue.
//!
//! This implementation is multi-threaded and operations between different consumer/client
//! components use different connections and can run in parallel.
//!
//! A single connection is shared by all instances of the same consumer component, identified
//! by its id (public key), so there may be some brief lock contention if several instances of
//! the same component (i.e. replicas) are simultaneously attempting to communicate with NATS.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_nats::jetstream::kv::Store;
use futures::{StreamExt, TryStreamExt};
use tokio::sync::{RwLock, Mutex};
use tokio::fs;
use tracing::{debug, error, warn, info, instrument};
use wascap::prelude::KeyPair;
use wasmcloud_provider_sdk::core::HostData;
use wasmcloud_provider_sdk::{
    get_connection, load_host_data, run_provider, Context, LinkConfig, Provider,
    propagate_trace_for_ctx,
};

mod config;
use config::NatsConnectionConfig;

use exports::wrpc::keyvalue;

wit_bindgen_wrpc::generate!();

type Result<T, E = keyvalue::store::Error> = core::result::Result<T, E>;

pub async fn run() -> anyhow::Result<()> {
    KvNatsProvider::run().await
}

/// [`NatsClientBundle`] holds a NATS client, and stream and locks information related to it.
#[derive(Debug, Clone)]
struct NatsClientBundle {
    pub client: async_nats::Client,
    pub js_context: async_nats::jetstream::Context,
    // Combination of the NATS server URI and the JetStream domain
    pub atomic_lock_suffix: String,
}

/// NATS implementation for wasi:keyvalue (via wrpc:keyvalue)
#[derive(Default, Clone)]
pub struct KvNatsProvider {
    consumer_components: Arc<RwLock<HashMap<String, NatsClientBundle>>>,
    // Ensuring thread-safety across invocations of the wrpc-keyvalue/atomic's `increment`
    // function, when the same NATS Kv bucket/key pair is used.
    // See the `increment` function for more details.
    atomic_locks: Arc<Mutex<HashMap<String, Arc<RwLock<()>>>>>,
    default_config: NatsConnectionConfig,
}
/// Implement the [`KvNatsProvider`] and [`Provider`] traits
impl KvNatsProvider {
    pub async fn run() -> anyhow::Result<()> {
        let host_data = load_host_data().context("failed to load host data")?;
        let provider = Self::from_host_data(host_data);
        let shutdown = run_provider(provider.clone(), "keyvalue-nats-provider")
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

    /// Build a [`KvNatsProvider`] from [`HostData`]
    pub fn from_host_data(host_data: &HostData) -> KvNatsProvider {
        let config = NatsConnectionConfig::from_map(&host_data.config);
        if let Ok(config) = config {
            KvNatsProvider {
                default_config: config,
                ..Default::default()
            }
        } else {
            warn!("Failed to build NATS connection configuration, falling back to default");
            KvNatsProvider::default()
        }
    }

    /// Attempt to connect to NATS url (with JWT credentials, if provided)
    async fn connect(
        &self,
        cfg: NatsConnectionConfig,
    ) -> anyhow::Result<NatsClientBundle> {
        let mut opts = match (cfg.auth_jwt, cfg.auth_seed) {
            (Some(jwt), Some(seed)) => {
                let seed = KeyPair::from_seed(&seed).context("failed to parse seed key pair")?;
                let seed = Arc::new(seed);
                async_nats::ConnectOptions::with_jwt(jwt, move |nonce| {
                    let seed = seed.clone();
                    async move { seed.sign(&nonce).map_err(async_nats::AuthError::new) }
                })
            }
            (None, None) => async_nats::ConnectOptions::default(),
            _ => bail!("must provide both jwt and seed for jwt authentication"),
        };
        if let Some(tls_ca) = &cfg.tls_ca {
            opts = add_tls_ca(tls_ca, opts)?;
        } else if let Some(tls_ca_file) = &cfg.tls_ca_file {
            let ca = fs::read_to_string(tls_ca_file)
                .await
                .context("failed to read TLS CA file")?;
            opts = add_tls_ca(&ca, opts)?;
        }

        // Get the cluster_uri
        let uri = cfg.cluster_uri.unwrap_or_default();

        // Connect to the NATS server
        let client = opts
            .name("NATS Key-Value Provider") // allow this to show up uniquely in a NATS connection list
            .connect(uri.clone())
            .await?;

        // Connect to JetStream
        let js_context = if let Some(domain) = cfg.js_domain.as_ref() {
            async_nats::jetstream::with_domain(client.clone(), domain)
        } else {
            async_nats::jetstream::new(client.clone())
        };

        // Set the atomic lock prefix
        let atomic_lock_suffix = format!("{}@{}", cfg.js_domain.unwrap_or_default(), uri);

        Ok(NatsClientBundle { client, js_context, atomic_lock_suffix })
    }

    /// Helper function to get the NATS client bundle from the client component's context
    async fn get_nats_client_bundle(&self, context: Option<Context>) -> anyhow::Result<NatsClientBundle> {
        if let Some(ref source_id) = context
            .as_ref()
            .and_then(|Context { component, .. }| component.clone())
        {
            let components = self.consumer_components.read().await;
            let nats_bundle = match components.get(source_id) {
                Some(nats_bundle) => nats_bundle,
                None => {
                    error!("consumer component not linked: {source_id}");
                    bail!("consumer component not linked: {source_id}")
                }
            };
            Ok((*nats_bundle).clone())
        } else {
            error!("no consumer component in request");
            bail!("no consumer component in request")
        }
    }

    /// Helper function to open an existing NATS Kv bucket from the client component's context
    #[instrument(level = "debug", skip_all)]
    async fn kv_store(
        &self,
        context: Option<Context>,
        bucket: String,
    ) -> Result<Store, keyvalue::store::Error> {
        let nats_bundle = self.get_nats_client_bundle(context).await
            .map_err(|e| keyvalue::store::Error::Other(e.to_string()))?;
        match nats_bundle.js_context.get_key_value(&bucket).await
        {
            Ok(store) => {
                info!(%bucket, "bucket opened");
                Ok(store)
            }
            Err(e) => {
                error!(%bucket, "failed to open bucket: {e:?}");
                Err(keyvalue::store::Error::Other(e.to_string()))
            }
        }
    }

    /// Helper function to get a value from the key-value store
    #[instrument(level = "debug", skip_all)]
    async fn get(&self, context: Option<Context>, bucket: String, key: String) -> anyhow::Result<Result<Option<Vec<u8>>>> {
        keyvalue::store::Handler::get(self, context, bucket, key).await
    }

    /// Helper function to set a value in the key-value store
    async fn set(&self, context: Option<Context>, bucket: String, key: String, value: Vec<u8>) -> anyhow::Result<Result<()>> {
        keyvalue::store::Handler::set(self, context, bucket, key, value).await
    }

    /// Helper function to delete a key-value pair from the key-value store
    async fn delete(&self, context: Option<Context>, bucket: String, key: String) -> anyhow::Result<Result<()>> {
        keyvalue::store::Handler::delete(self, context, bucket, key).await
    }
}

/// Handle provider control commands
impl Provider for KvNatsProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-component resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            source_id, config, ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let config = if config.is_empty() {
            self.default_config.clone()
        } else {
            // create a config from the supplied values and merge that with the existing default
            // NATS connection configuration
            match NatsConnectionConfig::from_map(config) {
                Ok(ncc) => self.default_config.merge(&ncc),
                Err(e) => {
                    error!("Failed to build NATS connection configuration: {e:?}");
                    return Err(anyhow!(e).context("failed to build NATS connection configuration"));
                }
            }
        };

        let mut consumer_components = self.consumer_components.write().await;
        let nats_bundle = match self.connect(config).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to connect to NATS: {e:?}");
                bail!(anyhow!(e).context("failed to connect to NATS"))
            }
        };
        consumer_components.insert(source_id.into(), nats_bundle);

        Ok(())
    }

    /// Provider should perform any operations needed for a link deletion, including cleaning up
    /// per-component resources.
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, source_id: &str) -> anyhow::Result<()> {
        let mut links = self.consumer_components.write().await;
        if let Some(bundle) = links.remove(source_id) {
            let client = &bundle.client;
            debug!(
                "droping NATS client [{}] for (consumer) component [{}]...",
                format!(
                    "{}:{}",
                    client.server_info().server_id,
                    client.server_info().client_id
                ),
                source_id
            );
        }

        debug!(
            "finished processing (consumer) link deletion for component [{}]",
            source_id
        );

        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> anyhow::Result<()> {
        // clear the consumer components
        let mut consumers = self.consumer_components.write().await;
        consumers.clear();

        Ok(())
    }
}

/// Implement the 'wasi:keyvalue/store' capability provider interface
impl keyvalue::store::Handler<Option<Context>> for KvNatsProvider {
    // Get the last revision of a value, for a given key, from the key-value store
    #[instrument(level = "debug", skip(self))]
    async fn get(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<Option<Vec<u8>>>> {
        propagate_trace_for_ctx!(context);

        match self.kv_store(context, bucket).await {
            Ok(store) => {
                match store.get(key.clone()).await {
                    Ok(Some(bytes)) => Ok(Ok(Some(bytes.to_vec()))),
                    Ok(None) => Ok(Ok(None)),
                    Err(err) => {
                      error!(%key, "failed to get key value: {err:?}");
                      Ok(Err(keyvalue::store::Error::Other(err.to_string())))
                    }
                }
            },
            Err(err) => Ok(Err(err)),
        }
    }

    // Set new key-value pair in the key-value store. If key didn’t exist, it is created. If it did exist, a new value with a new version is added
    #[instrument(level = "debug", skip(self))]
    async fn set(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
        value: Vec<u8>,
    ) -> anyhow::Result<Result<()>> {
        propagate_trace_for_ctx!(context);

        match self.kv_store(context, bucket).await {
            Ok(store) => {
                match store.put(key.clone(), bytes::Bytes::from(value)).await {
                Ok(_) => Ok(Ok(())),
                Err(err) => {
                    error!(%key, "failed to set key value: {err:?}");
                    Ok(Err(keyvalue::store::Error::Other(err.to_string())))
                }
                }
            },
            Err(err) => Ok(Err(err)),
        }
    }

    // Purge all the revisions of a key destructively,  from the key-value store, leaving behind a single purge entry in-place.
    #[instrument(level = "debug", skip(self))]
    async fn delete(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<()>> {
        propagate_trace_for_ctx!(context);

        match self.kv_store(context, bucket).await {
            Ok(store) => {
                match store.purge(key.clone()).await {
                Ok(_) => Ok(Ok(())),
                Err(err) => {
                    error!(%key, "failed to delete key: {err:?}");
                    Ok(Err(keyvalue::store::Error::Other(err.to_string())))
                }
                }
            },
            Err(err) => Ok(Err(err)),
        }
    }

    // Check if a key exists in the key-value store
    #[instrument(level = "debug", skip(self))]
    async fn exists(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<bool>> {
        propagate_trace_for_ctx!(context);

        match self.get(context, bucket, key).await {
          Ok(Ok(Some(_))) => Ok(Ok(true)),
          Ok(Ok(None)) => Ok(Ok(false)),
          Ok(Err(err)) => Ok(Err(err)),
          Err(err) => Ok(Err(keyvalue::store::Error::Other(err.to_string()))),
        }
    }

    // List all keys in the key-value store
    #[instrument(level = "debug", skip(self))]
    async fn list_keys(
        &self,
        context: Option<Context>,
        bucket: String,
        cursor: Option<u64>,
    ) -> anyhow::Result<Result<keyvalue::store::KeyResponse>> {
        propagate_trace_for_ctx!(context);

        match self.kv_store(context, bucket).await {
            Ok(store) => {
                match store.keys().await {
                    Ok(keys) => {
                        match keys.skip(cursor.unwrap_or(0) as usize).take(usize::MAX).try_collect().await {
                            Ok(keys) => Ok(Ok(keyvalue::store::KeyResponse {
                                keys,
                                cursor: None,
                            })),
                            Err(err) => {
                                error!("failed to list keys: {err:?}");
                                Ok(Err(keyvalue::store::Error::Other(err.to_string())))
                            }
                        }
                    },
                    Err(err) => {
                        error!("failed to list keys: {err:?}");
                        Ok(Err(keyvalue::store::Error::Other(err.to_string())))
                    },
                }
            },
            Err(err) => Ok(Err(err)),
        }
    }
}

/// Implement the 'wasi:keyvalue/atomic' capability provider interface
///
/// True atomic operations requires the `resource provider`, in this case NATS server, support.
/// NATS, however, does not support keyvalue store atomic operations; so to avoid depriving the
/// consumer components of the atomic increment operation, the implementation is using a
/// tokio::sync::RwLock, which will ensure atomicity of value increments given the following
/// constraints:
///  1. The same KvNatsProvider instance is used for the increment operation.
///  2. The same NATS server and Jetstream domain are targeted.
///  3. The same NATS Kv key-bucket pair is targeted.
impl keyvalue::atomics::Handler<Option<Context>> for KvNatsProvider {
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

        // Get or create the lock for this bucket:key pair
        let lock = {
          let mut locks = self.atomic_locks.lock().await;
          let lock_key = format!("{}-{}-{}", key, bucket, self.get_nats_client_bundle(context.clone()).await?.atomic_lock_suffix);
          locks.entry(lock_key).or_insert_with(|| Arc::new(RwLock::new(()))).clone()
        };

        // Lock the bucket:key pair
        let _permit = lock.write().await;

        // Get the current value
        let current_value = match self.get(context.clone(), bucket.clone(), key.clone()).await? {
            Ok(Some(value)) => {
                // Convert the value to a u64
                let value_str = std::str::from_utf8(&value)?;
                value_str.parse::<u64>()?
            }
            Ok(None) => 0,  // If the key doesn't exist, start from 0
            Err(err) => return Ok(Err(err)),
        };

        // Increment the value
        let new_value = current_value + delta;

        // Set the new value
        let _ = self.set(context, bucket, key, new_value.to_string().into_bytes()).await?;

        Ok(Ok(new_value))
    }
}

/// Implement the 'wasi:keyvalue/batch' capability provider interface
impl keyvalue::batch::Handler<Option<Context>> for KvNatsProvider {
    // Get multiple values from the key-value store
    #[instrument(level = "debug", skip(self))]
    async fn get_many(
        &self,
        ctx: Option<Context>,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<Result<Vec<Option<(String, Vec<u8>)>>>> {

        let ctx = ctx.clone();
        let bucket = bucket.clone();

        // Get the values for the keys
        let results: Result<Vec<_>, _> = keys
            .into_iter()
            .map(|key| {
                let ctx = ctx.clone();
                let bucket = bucket.clone();
                async move {
                self.get(ctx, bucket, key.clone()).await.map(|value| (key, value))
                }
            })
            .collect::<futures::stream::FuturesUnordered<_>>()
            .try_collect()
            .await;

        match results {
            Ok(values) => {
                let values: Result<Vec<_>, _> = values.into_iter().map(|(k, res)| match res {
                    Ok(Some(v)) => Ok(Some((k, v))),
                    Ok(None) => Ok(None),
                    Err(err) => {
                        error!("failed to parse key-value pairs: {err:?}");
                        Err(keyvalue::store::Error::Other(err.to_string()))
                    },
                }).collect();
                Ok(values)
            }
            Err(err) => {
                error!("failed to get many keys: {err:?}");
                Ok(Err(keyvalue::store::Error::Other(err.to_string())))
            },
        }
    }

    // Set multiple values in the key-value store
    #[instrument(level = "debug", skip(self))]
    async fn set_many(
        &self,
        ctx: Option<Context>,
        bucket: String,
        items: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<Result<()>> {

        let ctx = ctx.clone();
        let bucket = bucket.clone();

        // Set the values for the keys
        let results: Result<Vec<_>, _> = items
            .into_iter()
            .map(|(key, value)| {
                let ctx = ctx.clone();
                let bucket = bucket.clone();
                async move {
                self.set(ctx, bucket, key, value).await
                }
            })
            .collect::<futures::stream::FuturesUnordered<_>>()
            .try_collect()
            .await;

        // If all set operations were successful, return Ok(())
        results.map(|_| Ok(()))
    }

    // Delete multiple keys from the key-value store
    #[instrument(level = "debug", skip(self))]
    async fn delete_many(
        &self,
        ctx: Option<Context>,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<Result<()>> {

        let ctx = ctx.clone();
        let bucket = bucket.clone();

        // Delete the keys
        let results: Result<Vec<_>, _> = keys
            .into_iter()
            .map(|key| {
                let ctx = ctx.clone();
                let bucket = bucket.clone();
                async move {
                self.delete(ctx, bucket, key).await
                }
            })
            .collect::<futures::stream::FuturesUnordered<_>>()
            .try_collect()
            .await;

        // If all delete operations were successful, return Ok(())
        results.map(|_| Ok(()))
    }
}

/// Helper function for adding the TLS CA to the NATS connection options
fn add_tls_ca(
    tls_ca: &str,
    opts: async_nats::ConnectOptions,
) -> anyhow::Result<async_nats::ConnectOptions> {
    let ca = rustls_pemfile::read_one(&mut tls_ca.as_bytes()).context("failed to read CA")?;
    let mut roots = async_nats::rustls::RootCertStore::empty();
    if let Some(rustls_pemfile::Item::X509Certificate(ca)) = ca {
        roots.add_parsable_certificates(&[ca]);
    } else {
        bail!("tls ca: invalid certificate type, must be a DER encoded PEM file")
    };
    let tls_client = async_nats::rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(opts.tls_client_config(tls_client).require_tls(true))
}


// Performing various provider configuration tests
#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashMap;

    // Verify that a NatsConnectionConfig could be constructed from partial input
    #[test]
    fn test_default_connection_serialize() {
        let input = r#"
{
    "js_domain": "optional",
    "cluster_uri": "nats://super-cluster",
    "auth_jwt": "authy",
    "auth_seed": "seedy"
}
"#;

        let config: NatsConnectionConfig = serde_json::from_str(input).unwrap();
        assert_eq!(config.cluster_uri, Some("nats://super-cluster".to_string()));
        assert_eq!(config.js_domain, Some("optional".to_string()));
        assert_eq!(config.auth_jwt.unwrap(), "authy");
        assert_eq!(config.auth_seed.unwrap(), "seedy");
    }

    // Verify that two NatsConnectionConfigs could be merged
    #[test]
    fn test_connectionconfig_merge() {
        let ncc1 = NatsConnectionConfig {
            cluster_uri: Some("old_server".to_string()),
            ..Default::default()
        };
        let ncc2 = NatsConnectionConfig {
            js_domain: Some("new_domain".to_string()),
            cluster_uri: Some("server1".to_string()),
            auth_jwt: Some("jawty".to_string()),
            ..Default::default()
        };
        let ncc3 = ncc1.merge(&ncc2);
        assert_eq!(ncc3.js_domain, ncc2.js_domain);
        assert_eq!(ncc3.cluster_uri, ncc2.cluster_uri);
        assert_eq!(ncc3.auth_jwt, Some("jawty".to_string()));
    }

    // Verify that a NatsConnectionConfig could be constructed from a HashMap
    #[test]
    fn test_from_map_multiple_entries() -> anyhow::Result<()> {
        const CONFIG_NATS_CLIENT_JWT: &str = "client_jwt";
        const CONFIG_NATS_CLIENT_SEED: &str = "client_seed";
        let ncc = NatsConnectionConfig::from_map(&HashMap::from([
            ("tls_ca".to_string(), "rootCA".to_string()),
            ("js_domain".to_string(), "optional".to_string()),
            (CONFIG_NATS_CLIENT_JWT.to_string(), "authy".to_string()),
            (CONFIG_NATS_CLIENT_SEED.to_string(), "seedy".to_string()),
        ]))?;
        assert_eq!(ncc.tls_ca, Some("rootCA".to_string()));
        assert_eq!(ncc.js_domain, Some("optional".to_string()));
        assert_eq!(ncc.auth_jwt, Some("authy".to_string()));
        assert_eq!(ncc.auth_seed, Some("seedy".to_string()));
        Ok(())
    }

    // Verify that a default NatsConnectionConfig will be constructed from an empty HashMap
    #[test]
    fn test_from_map_empty() -> anyhow::Result<()> {
        let ncc = NatsConnectionConfig::from_map(&HashMap::new())?;
        assert_eq!(ncc, NatsConnectionConfig::default());
        Ok(())
    }

    // Verify that the NatsConnectionConfig's merge function prioritizes the new values over the old ones
    #[test]
    fn test_merge_non_default_values() {
      let ncc1 = NatsConnectionConfig {
        js_domain: Some("old_domain".to_string()),
        cluster_uri: Some("old_server".to_string()),
        auth_jwt: Some("old_jawty".to_string()),
        ..Default::default()
      };
      let ncc2 = NatsConnectionConfig {
        js_domain: Some("new_domain".to_string()),
        cluster_uri: Some("server1".to_string()),
        auth_jwt: Some("new_jawty".to_string()),
        ..Default::default()
      };
      let ncc3 = ncc1.merge(&ncc2);
      assert_eq!(ncc3.js_domain, ncc2.js_domain);
      assert_eq!(ncc3.cluster_uri, ncc2.cluster_uri);
      assert_eq!(ncc3.auth_jwt, ncc2.auth_jwt);
    }

    // Verify that tls_ca is set
    #[test]
    fn test_add_tls_ca() {
        let tls_ca = "-----BEGIN CERTIFICATE-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAwJwz\n-----END CERTIFICATE-----";
        let opts = async_nats::ConnectOptions::new();
        let opts = add_tls_ca(tls_ca, opts);
        assert!(opts.is_ok())
    }
}
