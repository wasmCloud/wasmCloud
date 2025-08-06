//! NATS implementation for wrpc:keyvalue.
//!
//! This implementation is multi-threaded and operations between different consumer/client
//! components use different connections and can run in parallel.
//!
//! A single connection is shared by all instances of the same consumer component, identified
//! by its id (public key), so there may be some brief lock contention if several instances of
//! the same component are simultaneously attempting to communicate with NATS.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use bytes::Bytes;
use futures::{StreamExt as _, TryStreamExt as _};
use tokio::fs;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument, warn};
use wascap::prelude::KeyPair;
use wasmcloud_provider_sdk::core::HostData;
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, load_host_data, propagate_trace_for_ctx,
    run_provider, serve_provider_exports, Context, LinkConfig, LinkDeleteInfo, Provider,
};

mod config;
use config::NatsConnectionConfig;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc:keyvalue/atomics@0.2.0-draft": generate,
            "wrpc:keyvalue/batch@0.2.0-draft": generate,
            "wrpc:keyvalue/store@0.2.0-draft": generate,
        }
    });
}
use bindings::exports::wrpc::keyvalue;

type Result<T, E = keyvalue::store::Error> = core::result::Result<T, E>;

pub async fn run() -> anyhow::Result<()> {
    KvNatsProvider::run().await
}

/// The `atomic::increment` function's exponential backoff base interval
const EXPONENTIAL_BACKOFF_BASE_INTERVAL: u64 = 5; // milliseconds

/// [`NatsKvStores`] holds the handles to opened NATS Kv Stores, and their respective identifiers.
type NatsKvStores = HashMap<String, async_nats::jetstream::kv::Store>;

/// NATS implementation for wasi:keyvalue (via wrpc:keyvalue)
#[derive(Default, Clone)]
pub struct KvNatsProvider {
    consumer_components: Arc<RwLock<HashMap<String, NatsKvStores>>>,
    default_config: NatsConnectionConfig,
}
/// Implement the [`KvNatsProvider`] and [`Provider`] traits
impl KvNatsProvider {
    pub async fn run() -> anyhow::Result<()> {
        let host_data = load_host_data().context("failed to load host data")?;
        let flamegraph_path = host_data
            .config
            .get("FLAMEGRAPH_PATH")
            .map(String::from)
            .or_else(|| std::env::var("PROVIDER_KEYVALUE_NATS_FLAMEGRAPH_PATH").ok());
        initialize_observability!("keyvalue-nats-provider", flamegraph_path);
        let provider = Self::from_host_data(host_data);
        let shutdown = run_provider(provider.clone(), "keyvalue-nats-provider")
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

    /// Build a [`KvNatsProvider`] from [`HostData`]
    pub fn from_host_data(host_data: &HostData) -> KvNatsProvider {
        let config =
            NatsConnectionConfig::from_config_and_secrets(&host_data.config, &host_data.secrets);
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
        link_cfg: &LinkConfig<'_>,
    ) -> anyhow::Result<async_nats::jetstream::kv::Store> {
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

        // Get the JetStream context based on js_domain
        let js_context = if let Some(domain) = &cfg.js_domain {
            async_nats::jetstream::with_domain(client.clone(), domain.clone())
        } else {
            async_nats::jetstream::new(client.clone())
        };

        // If bucket auto-creation was specified in the link configuration,
        // create a bucket
        if link_cfg
            .config
            .get("enable_bucket_auto_create")
            .is_some_and(|v| v.to_lowercase() == "true")
        {
            // Get the JetStream context based on js_domain
            if let Err(e) = js_context
                .create_key_value(async_nats::jetstream::kv::Config {
                    bucket: cfg.bucket.clone(),
                    ..Default::default()
                })
                .await
            {
                warn!("failed to auto create bucket [{}]: {e}", cfg.bucket);
            }
        };

        // Open the key-value store
        let store = js_context.get_key_value(&cfg.bucket).await?;
        info!(%cfg.bucket, "NATS Kv store opened");

        // Return the handle to the opened NATS Kv store
        Ok(store)
    }

    /// Helper function to lookup and return the NATS Kv store handle, from the client component's context
    async fn get_kv_store(
        &self,
        context: Option<Context>,
        bucket_id: String,
    ) -> Result<async_nats::jetstream::kv::Store, keyvalue::store::Error> {
        if let Some(ref source_id) = context
            .as_ref()
            .and_then(|Context { component, .. }| component.clone())
        {
            let components = self.consumer_components.read().await;
            let kv_stores = match components.get(source_id) {
                Some(kv_stores) => kv_stores,
                None => {
                    return Err(keyvalue::store::Error::Other(format!(
                        "consumer component not linked: {source_id}"
                    )));
                }
            };
            kv_stores.get(&bucket_id).cloned().ok_or_else(|| {
                keyvalue::store::Error::Other(format!(
                    "No NATS Kv store found for bucket id (link name): {bucket_id}"
                ))
            })
        } else {
            Err(keyvalue::store::Error::Other(
                "no consumer component in the request".to_string(),
            ))
        }
    }

    /// Helper function to get a value from the key-value store
    #[instrument(level = "debug", skip_all)]
    async fn get(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<Option<Bytes>>> {
        keyvalue::store::Handler::get(self, context, bucket, key).await
    }

    /// Helper function to set a value in the key-value store
    async fn set(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
        value: Bytes,
    ) -> anyhow::Result<Result<()>> {
        keyvalue::store::Handler::set(self, context, bucket, key, value).await
    }

    /// Helper function to delete a key-value pair from the key-value store
    async fn delete(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<()>> {
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
        link_config: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let nats_config = if link_config.config.is_empty() {
            self.default_config.clone()
        } else {
            // create a config from the supplied values and merge that with the existing default
            // NATS connection configuration
            match NatsConnectionConfig::from_config_and_secrets(
                link_config.config,
                link_config.secrets,
            ) {
                Ok(ncc) => self.default_config.merge(&ncc),
                Err(e) => {
                    error!("Failed to build NATS connection configuration: {e:?}");
                    return Err(anyhow!(e).context("failed to build NATS connection configuration"));
                }
            }
        };
        println!("NATS Kv configuration: {nats_config:?}");

        let LinkConfig {
            source_id,
            link_name,
            ..
        }: LinkConfig<'_> = link_config;

        let kv_store = match self.connect(nats_config, &link_config).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to connect to NATS: {e:?}");
                bail!(anyhow!(e).context("failed to connect to NATS"))
            }
        };

        let mut consumer_components = self.consumer_components.write().await;
        // Check if there's an existing hashmap for the source_id
        if let Some(existing_kv_stores) = consumer_components.get_mut(&source_id.to_string()) {
            // If so, insert the new kv_store into it
            existing_kv_stores.insert(link_name.into(), kv_store);
        } else {
            // Otherwise, create a new hashmap and insert it
            consumer_components.insert(
                source_id.into(),
                HashMap::from([(link_name.into(), kv_store)]),
            );
        }

        Ok(())
    }

    /// Provider should perform any operations needed for a link deletion, including cleaning up
    /// per-component resources.
    #[instrument(level = "info", skip_all, fields(source_id = info.get_source_id()))]
    async fn delete_link_as_target(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_source_id();
        let mut links = self.consumer_components.write().await;
        if let Some(kv_store) = links.remove(component_id) {
            debug!(
                component_id,
                "dropping NATS Kv store [{kv_store:?}] for (consumer) component...",
            );
        }

        debug!(component_id, "finished processing link deletion");

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
    ) -> anyhow::Result<Result<Option<Bytes>>> {
        propagate_trace_for_ctx!(context);

        match self.get_kv_store(context, bucket).await {
            Ok(store) => match store.get(key.clone()).await {
                Ok(Some(bytes)) => Ok(Ok(Some(bytes))),
                Ok(None) => Ok(Ok(None)),
                Err(err) => {
                    error!(%key, "failed to get key value: {err:?}");
                    Ok(Err(keyvalue::store::Error::Other(err.to_string())))
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
        value: Bytes,
    ) -> anyhow::Result<Result<()>> {
        propagate_trace_for_ctx!(context);

        match self.get_kv_store(context, bucket).await {
            Ok(store) => match store.put(key.clone(), value).await {
                Ok(_) => Ok(Ok(())),
                Err(err) => {
                    error!(%key, "failed to set key value: {err:?}");
                    Ok(Err(keyvalue::store::Error::Other(err.to_string())))
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

        match self.get_kv_store(context, bucket).await {
            Ok(store) => match store.purge(key.clone()).await {
                Ok(_) => Ok(Ok(())),
                Err(err) => {
                    error!(%key, "failed to delete key: {err:?}");
                    Ok(Err(keyvalue::store::Error::Other(err.to_string())))
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

        match self.get_kv_store(context, bucket).await {
            Ok(store) => match store.keys().await {
                Ok(keys) => {
                    match keys
                        .skip(cursor.unwrap_or(0) as usize)
                        .take(usize::MAX)
                        .try_collect()
                        .await
                    {
                        Ok(keys) => Ok(Ok(keyvalue::store::KeyResponse { keys, cursor: None })),
                        Err(err) => {
                            error!("failed to list keys: {err:?}");
                            Ok(Err(keyvalue::store::Error::Other(err.to_string())))
                        }
                    }
                }
                Err(err) => {
                    error!("failed to list keys: {err:?}");
                    Ok(Err(keyvalue::store::Error::Other(err.to_string())))
                }
            },
            Err(err) => Ok(Err(err)),
        }
    }
}

/// Implement the 'wasi:keyvalue/atomic' capability provider interface
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

        // Try to increment the value up to 5 times with exponential backoff
        let kv_store = self.get_kv_store(context.clone(), bucket.clone()).await?;

        let mut new_value = 0;
        let mut success = false;
        for attempt in 0..5 {
            // Get the latest entry from the key-value store
            let entry = kv_store.entry(key.clone()).await?;

            // Get the current value and revision
            let (current_value, revision) = match &entry {
                Some(entry) if !entry.value.is_empty() => {
                    let value_str = std::str::from_utf8(&entry.value)?;
                    match value_str.parse::<u64>() {
                        Ok(num) => (num, entry.revision),
                        Err(_) => {
                            return Err(keyvalue::store::Error::Other(
                                "Cannot increment a non-numerical value".to_string(),
                            )
                            .into())
                        }
                    }
                }
                _ => (0, entry.as_ref().map_or(0, |e| e.revision)),
            };

            new_value = current_value + delta;

            // Increment the value of the key
            match kv_store
                .update(key.clone(), new_value.to_string().into(), revision)
                .await
            {
                Ok(_) => {
                    success = true;
                    break; // Exit the loop on success
                }
                Err(_) => {
                    // Apply exponential backoff delay if the revision has changed (i.e. the key has been updated since the last read)
                    if attempt > 0 {
                        let wait_time = EXPONENTIAL_BACKOFF_BASE_INTERVAL * 2u64.pow(attempt - 1);
                        tokio::time::sleep(std::time::Duration::from_millis(wait_time)).await;
                    }
                }
            }
        }

        if success {
            Ok(Ok(new_value))
        } else {
            // If all attempts fail, let user know
            Ok(Err(keyvalue::store::Error::Other(
                "Failed to increment the value after 5 attempts".to_string(),
            )))
        }
    }
}

/// Reducing type complexity for the `get_many` function of wasi:keyvalue/batch
type KvResult = Vec<Option<(String, Bytes)>>;

/// Implement the 'wasi:keyvalue/batch' capability provider interface
impl keyvalue::batch::Handler<Option<Context>> for KvNatsProvider {
    // Get multiple values from the key-value store
    #[instrument(level = "debug", skip(self))]
    async fn get_many(
        &self,
        ctx: Option<Context>,
        bucket: String,
        keys: Vec<String>,
    ) -> anyhow::Result<Result<KvResult>> {
        let ctx = ctx.clone();
        let bucket = bucket.clone();

        // Get the values for the keys
        let results: Result<Vec<_>, _> = keys
            .into_iter()
            .map(|key| {
                let ctx = ctx.clone();
                let bucket = bucket.clone();
                async move {
                    self.get(ctx, bucket, key.clone())
                        .await
                        .map(|value| (key, value))
                }
            })
            .collect::<futures::stream::FuturesUnordered<_>>()
            .try_collect()
            .await;

        match results {
            Ok(values) => {
                let values: Result<Vec<_>, _> = values
                    .into_iter()
                    .map(|(k, res)| match res {
                        Ok(Some(v)) => Ok(Some((k, v))),
                        Ok(None) => Ok(None),
                        Err(err) => {
                            error!("failed to parse key-value pairs: {err:?}");
                            Err(keyvalue::store::Error::Other(err.to_string()))
                        }
                    })
                    .collect();
                Ok(values)
            }
            Err(err) => {
                error!("failed to get many keys: {err:?}");
                Ok(Err(keyvalue::store::Error::Other(err.to_string())))
            }
        }
    }

    // Set multiple values in the key-value store
    #[instrument(level = "debug", skip(self))]
    async fn set_many(
        &self,
        ctx: Option<Context>,
        bucket: String,
        items: Vec<(String, Bytes)>,
    ) -> anyhow::Result<Result<()>> {
        let ctx = ctx.clone();
        let bucket = bucket.clone();

        // Set the values for the keys
        let results: Result<Vec<_>, _> = items
            .into_iter()
            .map(|(key, value)| {
                let ctx = ctx.clone();
                let bucket = bucket.clone();
                async move { self.set(ctx, bucket, key, value).await }
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
                async move { self.delete(ctx, bucket, key).await }
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
        roots.add_parsable_certificates([ca]);
    } else {
        bail!("tls ca: invalid certificate type, must be a DER encoded PEM file")
    };
    let tls_client = async_nats::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(opts.tls_client_config(tls_client).require_tls(true))
}

// Performing various provider configuration tests
#[cfg(test)]
mod test {
    use super::*;

    // Verify that tls_ca is set
    #[test]
    fn test_add_tls_ca() {
        let tls_ca = "-----BEGIN CERTIFICATE-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAwJwz\n-----END CERTIFICATE-----";
        let opts = async_nats::ConnectOptions::new();
        let opts = add_tls_ca(tls_ca, opts);
        assert!(opts.is_ok())
    }
}
