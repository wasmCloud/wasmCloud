//! This provider implementation is multi-threaded and operations between different consumer/client
//! components use different connections and can run in parallel.
//!
//! A single connection is shared by all instances of the same consumer component, identified
//! by its id (public key), so there may be some brief lock contention if several instances of
//! the same component (i.e. replicas) are simultaneously attempting to communicate with NATS.

#![allow(clippy::type_complexity)]
use std::collections::HashMap;
use std::sync::Arc;

use crate::bindings::ext::exports::wrpc::extension::{
    configurable::{self, InterfaceConfig},
    manageable,
};
use anyhow::{anyhow, bail, Context as _};
use tokio::fs;
use tracing::{debug, error, info, instrument};
use wascap::prelude::KeyPair;
use wasmcloud_provider_sdk::{
    core::secrets::SecretValue,
    get_connection, initialize_observability, run_provider, serve_provider_exports,
    types::{BindRequest, BindResponse, HealthCheckResponse},
    Context,
};

use crate::config::{NatsConnectionConfig, DEFAULT_NATS_URI};
use crate::{NatsBlobstore, NatsBlobstoreProvider};
// Import the wrpc interface bindings
use wrpc_interface_blobstore::bindings;

/// Implement the [`NatsBlobstoreProvider`] and [`Provider`] traits
impl NatsBlobstoreProvider {
    pub fn name() -> &'static str {
        "nats-bucket-provider"
    }

    pub async fn run() -> anyhow::Result<()> {
        let (shutdown, quit_tx) = run_provider(Self::name(), None)
            .await
            .context("failed to run provider")?;
        let provider = NatsBlobstoreProvider::new(quit_tx);
        let connection = get_connection();
        let (main_client, ext_client) = connection.get_wrpc_clients_for_serving().await?;

        serve_provider_exports(
            &main_client,
            &ext_client,
            provider,
            shutdown,
            bindings::serve,
            crate::bindings::ext::serve,
        )
        .await
        .context("failed to serve provider exports")
    }

    /// Attempt to connect to NATS url (with JWT credentials, if provided)
    async fn connect(
        &self,
        cfg: NatsConnectionConfig,
    ) -> anyhow::Result<async_nats::jetstream::context::Context> {
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

        // Get the cluster_uri with proper default
        let uri = cfg.cluster_uri.unwrap_or(DEFAULT_NATS_URI.to_string());

        // Connect to the NATS server
        let client = opts
            .name("NATS Object Store Provider")
            .connect(uri.clone())
            .await?;

        // Get the JetStream context based on js_domain
        let jetstream = if let Some(domain) = &cfg.js_domain {
            async_nats::jetstream::with_domain(client.clone(), domain.clone())
        } else {
            async_nats::jetstream::new(client.clone())
        };

        debug!("opened NATS JetStream: {:?}", jetstream);
        debug!("NATS Connection Configuration: {:?}", client);

        // Return the handle to the opened NATS Object store
        Ok(jetstream)
    }

    /// Helper function to lookup and return the NATS JetStream connection handle, and container storage
    /// configuration, using the client component's context.
    /// This ensures consistent implementation across all functions that need to get the NATS Blobstore.
    pub(crate) async fn get_blobstore(
        &self,
        context: Option<Context>,
    ) -> anyhow::Result<NatsBlobstore> {
        if let Some((component_id, link_name)) =
            context
                .as_ref()
                .and_then(|ctx @ Context { component, .. }| {
                    component
                        .clone()
                        .map(|component_id| (component_id, ctx.link_name().to_string()))
                })
        {
            // Acquire a read lock on the consumer components and attempt to find the specified component_id
            let components = self.consumer_components.read().await;
            let nats_stores = components
                .get(&component_id)
                .ok_or_else(|| anyhow!("consumer component not linked: {}", component_id))?;

            // Get the NATS Object Store handle and its storage configuration
            nats_stores
                .get(&link_name)
                .cloned()
                .ok_or_else(|| anyhow!("no NATS Object Store found for link name: {}", &link_name))
        } else {
            // If the context is None, return an error indicating no consumer component in the request
            bail!("no consumer component found in the request")
        }
    }
}

impl manageable::Handler<Option<Context>> for NatsBlobstoreProvider {
    async fn bind(
        &self,
        _cx: Option<Context>,
        _req: BindRequest,
    ) -> anyhow::Result<Result<BindResponse, String>> {
        Ok(Ok(BindResponse {
            identity_token: None,
            provider_xkey: Some(get_connection().provider_xkey.public_key().into()),
        }))
    }

    async fn health_request(
        &self,
        _cx: Option<Context>,
    ) -> anyhow::Result<Result<HealthCheckResponse, String>> {
        Ok(Ok(HealthCheckResponse {
            healthy: true,
            message: Some("OK".to_string()),
        }))
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self, _cx: Option<Context>) -> anyhow::Result<Result<(), String>> {
        // clear the consumer components
        let mut consumers = self.consumer_components.write().await;
        consumers.clear();
        // Signal shutdown
        let _ = self.quit_tx.send(());
        Ok(Ok(()))
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

impl configurable::Handler<Option<Context>> for NatsBlobstoreProvider {
    #[instrument(level = "debug", skip_all)]
    async fn update_base_config(
        &self,
        _cx: Option<Context>,
        incoming_config: wasmcloud_provider_sdk::types::BaseConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let flamegraph_path = incoming_config
            .config
            .iter()
            .find(|(k, _)| k == "FLAMEGRAPH_PATH")
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var("PROVIDER_BLOBSTORE_NATS_FLAMEGRAPH_PATH").ok());
        initialize_observability!(Self::name(), flamegraph_path, incoming_config.config);

        let config_map: HashMap<String, String> = incoming_config.config.into_iter().collect();
        let secrets_map: HashMap<String, SecretValue> = incoming_config
            .secrets
            .into_iter()
            .map(|(k, v)| (k, v.into()))
            .collect();

        debug!("Received config update: {:?}", config_map);

        // Create a new config from the update values
        let new_config = match NatsConnectionConfig::from_link_config(&config_map, &secrets_map) {
            Result::Ok(config) => config,
            Result::Err(e) => {
                error!("Failed to parse configuration update: {}", e);
                return Ok(Err(format!("failed to parse configuration: {e}")));
            }
        };

        // Update default config
        *self.default_config.write().await = new_config.clone();

        // Create new NATS connection with updated config
        let new_jetstream = match self.connect(new_config.clone()).await {
            Result::Ok(js) => js,
            Result::Err(e) => {
                error!("Failed to connect with new configuration: {}", e);
                return Ok(Err(format!(
                    "failed to connect with new configuration: {e}"
                )));
            }
        };

        // Update all existing connections with the new configuration
        let mut components = self.consumer_components.write().await;
        for stores in components.values_mut() {
            for store in stores.values_mut() {
                // Use existing NatsConnectionConfig merge functionality
                let merged_config = NatsConnectionConfig {
                    storage_config: Some(store.storage_config.clone()),
                    ..Default::default()
                }
                .merge(&new_config);

                store.storage_config = merged_config.storage_config.unwrap_or_default();
                store.jetstream = new_jetstream.clone();
            }
        }

        info!("Successfully updated all NATS connections with new configuration");

        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn update_interface_export_config(
        &self,
        _cx: Option<Context>,
        source_id: String,
        link_name: String,
        link_config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config = if link_config.config.is_empty() {
            self.default_config.read().await.clone()
        } else {
            // create a config from the supplied values and merge that with the existing default
            // NATS connection configuration
            let config_map: HashMap<String, String> = link_config.config.into_iter().collect();
            let secrets_map: HashMap<String, SecretValue> = link_config
                .secrets
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, v.into()))
                .collect();
            match NatsConnectionConfig::from_link_config(&config_map, &secrets_map) {
                Ok(ncc) => self.default_config.read().await.merge(&ncc),
                Err(e) => {
                    error!("failed to build NATS connection configuration: {:?}", e);
                    return Err(anyhow!(e).context("failed to build NATS connection configuration"));
                }
            }
        };
        debug!("NATS Blobstore provider configuration: {:?}", config);

        let jetstream = match self.connect(config.clone()).await {
            Ok(b) => b,
            Err(e) => {
                error!("failed to connect to NATS: {:?}", e);
                bail!(anyhow!(e).context("failed to connect to NATS"))
            }
        };

        let mut consumer_components = self.consumer_components.write().await;
        // Check if there's an existing hashmap for the source_id
        if let Some(existing_nats_stores) = consumer_components.get_mut(&source_id.to_string()) {
            // If so, insert the new jetstream into it
            existing_nats_stores.insert(
                link_name.into(),
                NatsBlobstore {
                    jetstream,
                    storage_config: config.storage_config.unwrap_or_default(),
                },
            );
        } else {
            // Otherwise, create a new hashmap and insert it
            consumer_components.insert(
                source_id.into(),
                HashMap::from([(
                    link_name.into(),
                    NatsBlobstore {
                        jetstream,
                        storage_config: config.storage_config.unwrap_or_default(),
                    },
                )]),
            );
        }

        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(target_id))]
    async fn update_interface_import_config(
        &self,
        _cx: Option<Context>,
        _target_id: String,
        _link_name: String,
        _config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(target_id))]
    async fn delete_interface_import_config(
        &self,
        _cx: Option<Context>,
        _target_id: String,
        _link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }

    #[instrument(level = "info", skip_all, fields(source_id))]
    async fn delete_interface_export_config(
        &self,
        _cx: Option<Context>,
        source_id: String,
        link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        let mut links = self.consumer_components.write().await;

        if let Some(nats_stores) = links.get_mut(&source_id) {
            if nats_stores.remove(&link_name).is_some() {
                debug!(
                    source_id,
                    link_name, "removed NATS JetStream connection for link name"
                );
            }

            // If the inner hashmap is empty, remove the source_id from the outer hashmap
            if nats_stores.is_empty() {
                links.remove(&source_id);
                debug!(
                    source_id,
                    "removed source_id from consumer components as it has no more link names"
                );
            }
        } else {
            debug!(source_id, "source_id not found in consumer components");
        }

        debug!(source_id, "finished processing link deletion");
        Ok(Ok(()))
    }
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
