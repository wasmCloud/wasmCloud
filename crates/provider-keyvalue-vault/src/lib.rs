use core::str;

use std::collections::{hash_map, HashMap};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _, Result};
use async_trait::async_trait;
use base64::Engine as _;
use bytes::{Bytes, BytesMut};
use futures::{Stream, TryStreamExt};
use tokio::sync::RwLock;
use tracing::{debug, error, instrument};
use wasmcloud_provider_sdk::interfaces::keyvalue::{Atomic, Eventual};
use wasmcloud_provider_sdk::{Context, LinkConfig, ProviderHandler, ProviderOperationResult};
use wrpc_transport::{AcceptedInvocation, Transmitter};

pub(crate) mod client;
pub(crate) mod config;

use crate::client::Client;
use crate::config::Config;

/// Token to indicate string data was passed during set
pub const STRING_VALUE_MARKER: &str = "string_data___";

/// Redis KV provider implementation which utilizes [Hashicorp Vault](https://developer.hashicorp.com/vault/docs)
#[derive(Default, Clone)]
pub struct KvVaultProvider {
    // store vault connection per actor
    actors: Arc<RwLock<HashMap<String, Arc<Client>>>>,
}

impl KvVaultProvider {
    /// Retrieve a client for a given context (determined by source_id)
    async fn get_client(&self, ctx: Option<Context>) -> Result<Arc<Client>> {
        let ctx = ctx.context("invocation context missing")?;
        // get the actor ID
        let source_id = ctx
            .actor
            .as_ref()
            .context("invalid parameter: no actor in request")?;

        // Clone the existing client for the given actor from the internal hash map
        let client = self
            .actors
            .read()
            .await
            .get(source_id)
            .with_context(|| format!("invalid parameter: actor [{source_id}] not linked"))?
            .clone();
        Ok(client)
    }

    /// Gets a value for a specified key. Deserialize the value as json
    /// If it's any other map, the entire map is returned as a serialized json string
    /// If the stored value is a plain string, returns the plain value
    /// All other values are returned as serialized json
    #[instrument(level = "debug", skip(ctx, self))]
    async fn get(
        &self,
        ctx: Option<Context>,
        path: String,
        key: String,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        let client = match self.get_client(ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                bail!("failed to retrieve client: {e}");
            }
        };
        match client.read_secret(&path).await {
            Ok(Some(mut secret)) => match secret.remove(&key) {
                Some(value) => {
                    let value = base64::engine::general_purpose::STANDARD_NO_PAD
                        .decode(value)
                        .context("failed to decode secret")?;
                    Ok(Some(value))
                }
                None => Ok(None),
            },
            Ok(None) => Ok(None),
            Err(e) => {
                error!(error = %e, "failed to read secret");
                bail!(anyhow!(e).context("failed to read secret"))
            }
        }
    }

    /// Returns true if the store contains the key
    #[instrument(level = "debug", skip(ctx, self))]
    async fn contains(
        &self,
        ctx: Option<Context>,
        path: String,
        key: String,
    ) -> anyhow::Result<bool> {
        let client = match self.get_client(ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                bail!("failed to retrieve client: {e}");
            }
        };
        match client.read_secret(&path).await {
            Ok(Some(secret)) => Ok(secret.contains_key(&key)),
            Ok(None) => Ok(false),
            Err(e) => {
                error!(error = %e, "failed to read secret");
                bail!(anyhow!(e).context("failed to read secret"))
            }
        }
    }

    /// Deletes a key from a secret
    #[instrument(level = "debug", skip(ctx, self))]
    async fn del(&self, ctx: Option<Context>, path: String, key: String) -> anyhow::Result<()> {
        let client = match self.get_client(ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                bail!("failed to retrieve client: {e}");
            }
        };
        let value = match client.read_secret(&path).await {
            Ok(Some(mut secret)) => {
                if secret.remove(&key).is_none() {
                    debug!("key does not exist in the secret");
                    return Ok(());
                }
                secret
            }
            Ok(None) => {
                debug!("secret not found");
                return Ok(());
            }
            Err(e) => {
                error!(error = %e, "failed to read secret");
                bail!(anyhow!(e).context("failed to read secret"))
            }
        };
        match client.write_secret(&path, &value).await {
            Ok(metadata) => {
                debug!(?metadata, "set returned metadata");
                Ok(())
            }
            Err(e) => {
                error!(error = %e, "failed to set secret");
                bail!(anyhow!(e).context("failed to set secret"))
            }
        }
    }

    /// Sets the value of a key.
    #[instrument(level = "debug", skip(ctx, self))]
    async fn set(
        &self,
        ctx: Option<Context>,
        path: String,
        key: String,
        value: Bytes,
    ) -> anyhow::Result<()> {
        let client = match self.get_client(ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                bail!("failed to retrieve client: {e}");
            }
        };
        let value = base64::engine::general_purpose::STANDARD_NO_PAD.encode(value);
        let value = match client.read_secret(&path).await {
            Ok(Some(mut secret)) => {
                match secret.entry(key) {
                    hash_map::Entry::Vacant(e) => {
                        e.insert(value);
                    }
                    hash_map::Entry::Occupied(mut e) => {
                        if *e.get() == value {
                            return Ok(());
                        } else {
                            e.insert(value);
                        }
                    }
                }
                secret
            }
            Ok(None) => HashMap::from([(key, value)]),
            Err(e) => {
                error!(error = %e, "vault read: other error");
                bail!(anyhow!(e).context("vault read: other error"))
            }
        };
        match client.write_secret(&path, &value).await {
            Ok(metadata) => {
                debug!(?metadata, "set returned metadata");
                Ok(())
            }
            Err(e) => {
                error!(error = %e, "failed to set secret");
                bail!(anyhow!(e).context("failed to set secret"))
            }
        }
    }
}

impl Eventual for KvVaultProvider {
    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_delete<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (bucket, key),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, (String, String), Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(result_subject, self.del(context, bucket, key).await)
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_exists<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (bucket, key),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, (String, String), Tx>,
    ) {
        if let Err(err) = transmitter
            .transmit_static(result_subject, self.contains(context, bucket, key).await)
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_get<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (bucket, key),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, (String, String), Tx>,
    ) {
        let value = match self.get(context, bucket, key).await {
            Ok(Some(value)) => Ok(Some(Some(value))),
            Ok(None) => Ok(None),
            Err(err) => Err(err),
        };
        if let Err(err) = transmitter.transmit_static(result_subject, value).await {
            error!(?err, "failed to transmit result")
        }
    }

    #[instrument(
        level = "debug",
        skip(self, result_subject, error_subject, value, transmitter)
    )]
    async fn serve_set<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (bucket, key, value),
            error_subject,
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<Context>,
            (String, String, impl Stream<Item = anyhow::Result<Bytes>>),
            Tx,
        >,
    ) {
        let value: BytesMut = match value.try_collect().await {
            Ok(value) => value,
            Err(err) => {
                error!(?err, "failed to receive value");
                if let Err(err) = transmitter
                    .transmit_static(error_subject, err.to_string())
                    .await
                {
                    error!(?err, "failed to transmit error")
                }
                return;
            }
        };
        if let Err(err) = transmitter
            .transmit_static(
                result_subject,
                self.set(context, bucket, key, value.freeze()).await,
            )
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }
}

impl Atomic for KvVaultProvider {
    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_compare_and_swap<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (bucket, key, old, new),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, (String, String, u64, u64), Tx>,
    ) {
        // TODO: Use bucket
        _ = bucket;
        if let Err(err) = transmitter
            .transmit_static(result_subject, Err::<(), _>("not supported"))
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }

    /// Increments a numeric value, returning the new value
    #[instrument(level = "debug", skip(self, result_subject, transmitter))]
    async fn serve_increment<Tx: Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (bucket, key, value),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<Option<Context>, (String, String, u64), Tx>,
    ) {
        // TODO: Use bucket
        _ = bucket;
        if let Err(err) = transmitter
            .transmit_static(result_subject, Err::<(), _>("not supported"))
            .await
        {
            error!(?err, "failed to transmit result")
        }
    }
}

/// Handle provider control commands, the minimum required of any provider on
/// a wasmcloud lattice
#[async_trait]
impl ProviderHandler for KvVaultProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip_all, fields(source_id = %link_config.get_source_id()))]
    async fn receive_link_config_as_target(
        &self,
        link_config: impl LinkConfig,
    ) -> ProviderOperationResult<()> {
        let source_id = link_config.get_source_id();
        let link_name = link_config.get_source_id();
        debug!(
           %source_id,
           %link_name,
            "adding link for actor",
        );

        let config_values = link_config.get_config();
        let config = match Config::from_values(config_values) {
            Ok(config) => config,
            Err(e) => {
                error!(
                    %source_id,
                    %link_name,
                    "failed to parse config: {e}",
                );
                return Err(anyhow!(e).context("failed to parse config").into());
            }
        };

        let client = match Client::new(config.clone()) {
            Ok(client) => client,
            Err(e) => {
                error!(
                    %source_id,
                    %link_name,
                    "failed to create new client config: {e}",
                );
                return Err(anyhow!(e)
                    .context("failed to create new client config")
                    .into());
            }
        };
        client.set_renewal().await;

        let mut update_map = self.actors.write().await;
        update_map.insert(source_id.to_string(), Arc::new(client));

        Ok(())
    }

    /// Handle notification that a link is dropped - close the connection
    #[instrument(level = "debug", skip(self))]
    async fn delete_link(&self, source_id: &str) -> ProviderOperationResult<()> {
        let mut aw = self.actors.write().await;
        if let Some(client) = aw.remove(source_id) {
            debug!("deleting link for actor [{source_id}]");
            drop(client)
        }
        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> ProviderOperationResult<()> {
        let mut aw = self.actors.write().await;
        // Empty the actor link data and stop all servers
        for (_, client) in aw.drain() {
            drop(client)
        }
        Ok(())
    }
}
