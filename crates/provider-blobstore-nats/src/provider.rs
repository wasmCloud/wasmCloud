//! This provider implementation is multi-threaded and operations between different consumer/client
//! components use different connections and can run in parallel.
//!
//! A single connection is shared by all instances of the same consumer component, identified
//! by its id (public key), so there may be some brief lock contention if several instances of
//! the same component (i.e. replicas) are simultaneously attempting to communicate with NATS.

#![allow(clippy::type_complexity)]
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use core::future::Future;
use core::pin::Pin;
use futures::Stream;
use tokio::fs;
use tracing::{debug, error, info, instrument, warn};
use wascap::prelude::KeyPair;
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, load_host_data, run_provider, serve_provider_exports,
    Context, HostData, LinkConfig, LinkDeleteInfo, Provider, ProviderConfigUpdate,
};

use crate::config::{NatsConnectionConfig, DEFAULT_NATS_URI};
use crate::{NatsBlobstore, NatsBlobstoreProvider};
// Import the wrpc interface bindings
use wrpc_interface_blobstore::bindings;

/// Implement the [`NatsBlobstoreProvider`] and [`Provider`] traits
impl NatsBlobstoreProvider {
    pub async fn run() -> anyhow::Result<()> {
        let host_data = load_host_data().context("failed to load host data")?;
        let provider = Self::from_host_data(host_data);
        let shutdown = run_provider(provider.clone(), "nats-bucket-provider")
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        let flamegraph_path = host_data
            .config
            .get("FLAMEGRAPH_PATH")
            .map(String::from)
            .or_else(|| std::env::var("PROVIDER_BLOBSTORE_NATS_FLAMEGRAPH_PATH").ok());
        initialize_observability!("blobstore-nats-provider", flamegraph_path);
        serve_provider_exports(
            &connection
                .get_wrpc_client(connection.provider_key())
                .await?,
            provider,
            shutdown,
            bindings::serve,
        )
        .await
        .context("failed to serve provider exports")
    }

    /// Build a [`NatsBlobstoreProvider`] from [`HostData`]
    pub fn from_host_data(host_data: &HostData) -> NatsBlobstoreProvider {
        let config = NatsConnectionConfig::from_link_config(&host_data.config, &host_data.secrets);
        if let Ok(default_config) = config {
            NatsBlobstoreProvider {
                default_config,
                ..Default::default()
            }
        } else {
            warn!("failed to build NATS connection configuration, falling back to default");
            NatsBlobstoreProvider::default()
        }
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

    /// Helper function to list all objects in a NATS blobstore container.
    /// This ensures consistent implementation across all functions that need to list objects.
    /// Delegates to the core implementation in blobstore.rs.
    #[instrument(level = "debug", skip_all)]
    async fn list_container_objects(
        &self,
        context: Option<Context>,
        name: String,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> anyhow::Result<
        Result<
            (
                Pin<Box<dyn Stream<Item = Vec<String>> + Send>>,
                Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
            ),
            String,
        >,
    > {
        bindings::exports::wrpc::blobstore::blobstore::Handler::list_container_objects(
            self, context, name, offset, limit,
        )
        .await
    }

    /// Help function to delete an object from a NATS blobstore container.
    /// This ensures consistent implementation across all functions that need to delete objects.
    /// Delegates to the core implementation in blobstore.rs.
    #[instrument(level = "debug", skip_all)]
    async fn delete_object(
        &self,
        context: Option<Context>,
        id: bindings::wrpc::blobstore::types::ObjectId,
    ) -> anyhow::Result<Result<(), String>> {
        bindings::exports::wrpc::blobstore::blobstore::Handler::delete_object(self, context, id)
            .await
    }

    /// Help function to delete a collection of objects from a NATS blobstore container
    #[instrument(level = "debug", skip_all)]
    async fn delete_objects(
        &self,
        context: Option<Context>,
        name: String,
        objects: Vec<String>,
    ) -> anyhow::Result<Result<(), String>> {
        bindings::exports::wrpc::blobstore::blobstore::Handler::delete_objects(
            self, context, name, objects,
        )
        .await
    }
}

/// Handle provider control commands
impl Provider for NatsBlobstoreProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-component resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn receive_link_config_as_target(
        &self,
        link_config: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let LinkConfig {
            source_id,
            link_name,
            ..
        } = link_config;

        let config = if link_config.config.is_empty() {
            self.default_config.clone()
        } else {
            // create a config from the supplied values and merge that with the existing default
            // NATS connection configuration
            match NatsConnectionConfig::from_link_config(link_config.config, link_config.secrets) {
                Ok(ncc) => self.default_config.merge(&ncc),
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

        Ok(())
    }

    /// Provider should perform any operations needed for a link deletion, including cleaning up
    /// per-component resources.
    #[instrument(level = "info", skip_all, fields(source_id = info.get_source_id(), link_name = info.get_link_name()))]
    async fn delete_link_as_target(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let source_id = info.get_source_id();
        let link_name = info.get_link_name();
        let mut links = self.consumer_components.write().await;

        if let Some(nats_stores) = links.get_mut(source_id) {
            if nats_stores.remove(link_name).is_some() {
                debug!(
                    source_id,
                    link_name, "removed NATS JetStream connection for link name"
                );
            }

            // If the inner hashmap is empty, remove the source_id from the outer hashmap
            if nats_stores.is_empty() {
                links.remove(source_id);
                debug!(
                    source_id,
                    "removed source_id from consumer components as it has no more link names"
                );
            }
        } else {
            debug!(source_id, "source_id not found in consumer components");
        }

        debug!(source_id, "finished processing link deletion");

        Ok(())
    }

    /// Provider should perform any operations needed for configuration updates, including cleaning up
    /// invalidated link resources.
    #[instrument(level = "debug", skip_all, fields(link_name))]
    async fn on_config_update(&self, update: impl ProviderConfigUpdate) -> anyhow::Result<()> {
        let values = update.get_values();
        debug!("Received config update: {:?}", values);

        // Create a new config from the update values
        let new_config = match NatsConnectionConfig::from_link_config(values, &HashMap::new()) {
            Ok(config) => config,
            Err(e) => {
                error!("Failed to parse configuration update: {}", e);
                return Ok(());
            }
        };

        // Create new NATS connection with updated config
        let new_jetstream = match self.connect(new_config.clone()).await {
            Ok(js) => js,
            Err(e) => {
                error!("Failed to connect with new configuration: {}", e);
                return Ok(());
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
