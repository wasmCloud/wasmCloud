pub(crate) mod config;

use core::str;
use core::time::Duration;

use std::collections::{hash_map, HashMap};
use std::string::ToString;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use base64::Engine as _;
use bytes::Bytes;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, instrument, warn};
use vaultrs::client::{Client as _, VaultClient, VaultClientSettings};
use wasmcloud_provider_sdk::{
    get_connection, propagate_trace_for_ctx, run_provider, Context, LinkConfig, Provider,
};
use wasmcloud_provider_sdk::{initialize_observability, serve_provider_exports};

use crate::config::Config;

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wrpc:keyvalue/store@0.2.0-draft": generate,
        }
    });
}
use bindings::exports::wrpc::keyvalue;

type Result<T, E = keyvalue::store::Error> = core::result::Result<T, E>;

/// Vault HTTP api version. As of Vault 1.9.x (Feb 2022), all http api calls use version 1
const API_VERSION: u8 = 1;

/// Default TTL for tokens used by this provider. Defaults to 72 hours.
pub const TOKEN_INCREMENT_TTL: &str = "72h";
pub const TOKEN_REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60 * 12); // 12 hours

pub async fn run() -> anyhow::Result<()> {
    KvVaultProvider::run().await
}

/// Vault client connection information.
#[derive(Clone)]
pub struct Client {
    inner: Arc<vaultrs::client::VaultClient>,
    namespace: String,
    token_increment_ttl: String,
    token_refresh_interval: Duration,
    renew_task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl Client {
    /// Creates a new Vault client. See [config](./config.rs) for explanation of parameters.
    ///
    /// Note that this constructor does not attempt to connect to the vault server,
    /// so the vault server does not need to be running at the time a `LinkDefinition` to this provider is created.
    pub fn new(config: Config) -> Result<Self, vaultrs::error::ClientError> {
        let client = VaultClient::new(VaultClientSettings {
            token: config.token,
            address: config.addr,
            ca_certs: config.certs,
            verify: false,
            version: API_VERSION,
            wrapping: false,
            timeout: None,
            namespace: None,
            identity: None,
        })?;
        Ok(Self {
            inner: Arc::new(client),
            namespace: config.mount,
            token_increment_ttl: config
                .token_increment_ttl
                .unwrap_or(TOKEN_INCREMENT_TTL.into()),
            token_refresh_interval: config
                .token_refresh_interval
                .unwrap_or(TOKEN_REFRESH_INTERVAL),
            renew_task: Arc::default(),
        })
    }

    /// Reads value of secret using namespace and key path
    pub async fn read_secret(&self, path: &str) -> Result<Option<HashMap<String, String>>> {
        match vaultrs::kv2::read(self.inner.as_ref(), &self.namespace, path).await {
            Err(vaultrs::error::ClientError::APIError {
                code: 404,
                errors: _,
            }) => Ok(None),
            Err(err) => {
                error!(error = %err, "failed to read secret");
                Err(keyvalue::store::Error::Other(format!(
                    "{:#}",
                    anyhow!(err).context("failed to read secret")
                )))
            }
            Ok(val) => Ok(val),
        }
    }

    /// Writes value of secret using namespace and key path
    pub async fn write_secret(&self, path: &str, data: &HashMap<String, String>) -> Result<()> {
        let md = vaultrs::kv2::set(self.inner.as_ref(), &self.namespace, path, data)
            .await
            .map_err(|err| {
                error!(error = %err, "failed to write secret");
                keyvalue::store::Error::Other(format!(
                    "{:#}",
                    anyhow!(err).context("failed to write secret")
                ))
            })?;
        debug!(?md, "set returned metadata");
        Ok(())
    }

    /// Sets up a background task to renew the token at the configured interval. This function
    /// attempts to lock the `renew_task` mutex and will deadlock if called without first ensuring
    /// the lock is available.
    pub async fn set_renewal(&self) {
        let mut renew_task = self.renew_task.lock().await;
        if let Some(handle) = renew_task.take() {
            handle.abort();
        }
        let client = self.inner.clone();
        let interval = self.token_refresh_interval;
        let ttl = self.token_increment_ttl.clone();

        *renew_task = Some(tokio::spawn(async move {
            let mut next_interval = tokio::time::interval(interval);
            loop {
                next_interval.tick().await;
                // NOTE(brooksmtownsend): Errors are appropriately logged in the function
                let _ = renew_self(&client, ttl.as_str()).await;
            }
        }));
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // NOTE(brooksmtownsend): We're trying to lock here so we don't deadlock on dropping.
        if let Ok(mut renew_task) = self.renew_task.try_lock() {
            if let Some(handle) = renew_task.take() {
                handle.abort();
            }
        }
    }
}

/// Helper function to renew a client's token, incrementing the validity by `increment`
async fn renew_self(
    client: &VaultClient,
    increment: &str,
) -> Result<(), vaultrs::error::ClientError> {
    debug!("renewing token");
    client.renew(Some(increment)).await.map_err(|e| {
        error!("error renewing self token: {}", e);
        e
    })?;

    let info = client.lookup().await.map_err(|e| {
        error!("error looking up self token: {}", e);
        e
    })?;

    let expire_time = info.expire_time.unwrap_or_else(|| "None".to_string());
    info!(%expire_time, accessor = %info.accessor, "renewed token");
    Ok(())
}

/// Redis KV provider implementation which utilizes [Hashicorp Vault](https://developer.hashicorp.com/vault/docs)
#[derive(Default, Clone)]
pub struct KvVaultProvider {
    // store vault connection per component
    components: Arc<RwLock<HashMap<String, Arc<Client>>>>,
}

impl KvVaultProvider {
    pub fn name() -> &'static str {
        "keyvalue-vault-provider"
    }

    pub async fn run() -> anyhow::Result<()> {
        initialize_observability!(
            KvVaultProvider::name(),
            std::env::var_os("PROVIDER_KV_VAULT_FLAMEGRAPH_PATH")
        );

        let provider = Self::default();
        let shutdown = run_provider(provider.clone(), KvVaultProvider::name())
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

    /// Retrieve a client for a given context (determined by `source_id`)
    async fn get_client(&self, ctx: Option<Context>) -> Result<Arc<Client>> {
        let ctx = ctx.ok_or_else(|| {
            warn!("invocation context missing");
            keyvalue::store::Error::Other("invocation context missing".into())
        })?;
        let source_id = ctx.component.as_ref().ok_or_else(|| {
            warn!("source ID missing");
            keyvalue::store::Error::Other("source ID missing".into())
        })?;
        let links = self.components.read().await;
        links.get(source_id).cloned().ok_or_else(|| {
            warn!(source_id, "source ID not linked");
            keyvalue::store::Error::Other("source ID not linked".into())
        })
    }

    /// Gets a value for a specified key. Deserialize the value as json
    /// If it's any other map, the entire map is returned as a serialized json string
    /// If the stored value is a plain string, returns the plain value
    /// All other values are returned as serialized json
    #[instrument(level = "debug", skip(ctx, self))]
    async fn get(&self, ctx: Option<Context>, path: String, key: String) -> Result<Option<Bytes>> {
        propagate_trace_for_ctx!(ctx);
        let client = self.get_client(ctx).await?;
        if let Some(mut secret) = client.read_secret(&path).await? {
            match secret.remove(&key) {
                Some(value) => {
                    let value = base64::engine::general_purpose::STANDARD_NO_PAD
                        .decode(value)
                        .map_err(|err| {
                            error!(?err, "failed to decode secret value");
                            keyvalue::store::Error::Other(format!(
                                "{:#}",
                                anyhow!(err).context("failed to decode secret value")
                            ))
                        })?;
                    Ok(Some(value.into()))
                }
                None => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    /// Returns true if the store contains the key
    #[instrument(level = "debug", skip(ctx, self))]
    async fn contains(&self, ctx: Option<Context>, path: String, key: String) -> Result<bool> {
        propagate_trace_for_ctx!(ctx);
        let client = self.get_client(ctx).await?;
        let secret = client.read_secret(&path).await?;
        Ok(secret.is_some_and(|secret| secret.contains_key(&key)))
    }

    /// Deletes a key from a secret
    #[instrument(level = "debug", skip(ctx, self))]
    async fn del(&self, ctx: Option<Context>, path: String, key: String) -> Result<()> {
        propagate_trace_for_ctx!(ctx);
        let client = self.get_client(ctx).await?;
        let secret = client.read_secret(&path).await?;
        let secret = if let Some(mut secret) = secret {
            if secret.remove(&key).is_none() {
                debug!("key does not exist in the secret");
                return Ok(());
            }
            secret
        } else {
            debug!("secret not found");
            return Ok(());
        };
        client.write_secret(&path, &secret).await
    }

    /// Sets the value of a key.
    #[instrument(level = "debug", skip(ctx, self))]
    async fn set(
        &self,
        ctx: Option<Context>,
        path: String,
        key: String,
        value: Bytes,
    ) -> Result<()> {
        propagate_trace_for_ctx!(ctx);
        let client = self.get_client(ctx).await?;
        let value = base64::engine::general_purpose::STANDARD_NO_PAD.encode(value);
        let secret = client.read_secret(&path).await?;
        let secret = if let Some(mut secret) = secret {
            match secret.entry(key) {
                hash_map::Entry::Vacant(e) => {
                    e.insert(value);
                }
                hash_map::Entry::Occupied(mut e) => {
                    if *e.get() == value {
                        return Ok(());
                    }
                    e.insert(value);
                }
            }
            secret
        } else {
            HashMap::from([(key, value)])
        };
        client.write_secret(&path, &secret).await
    }

    #[instrument(level = "debug", skip(ctx, self))]
    async fn list_keys(
        &self,
        ctx: Option<Context>,
        path: String,
        skip: u64,
    ) -> Result<keyvalue::store::KeyResponse> {
        propagate_trace_for_ctx!(ctx);
        let client = self.get_client(ctx).await?;
        let secret = client.read_secret(&path).await?;
        Ok(keyvalue::store::KeyResponse {
            cursor: None,
            keys: secret
                .map(|secret| {
                    secret
                        .into_keys()
                        .skip(skip.try_into().unwrap_or(usize::MAX))
                        .collect()
                })
                .unwrap_or_default(),
        })
    }
}

impl keyvalue::store::Handler<Option<Context>> for KvVaultProvider {
    #[instrument(level = "debug", skip(self))]
    async fn delete(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<()>> {
        propagate_trace_for_ctx!(context);
        Ok(self.del(context, bucket, key).await)
    }

    #[instrument(level = "debug", skip(self))]
    async fn exists(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<bool>> {
        propagate_trace_for_ctx!(context);
        Ok(self.contains(context, bucket, key).await)
    }

    #[instrument(level = "debug", skip(self))]
    async fn get(
        &self,
        context: Option<Context>,
        bucket: String,
        key: String,
    ) -> anyhow::Result<Result<Option<Bytes>>> {
        propagate_trace_for_ctx!(context);
        Ok(self.get(context, bucket, key).await)
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
        Ok(self.set(context, bucket, key, value).await)
    }

    #[instrument(level = "debug", skip(self))]
    async fn list_keys(
        &self,
        context: Option<Context>,
        bucket: String,
        cursor: Option<u64>,
    ) -> anyhow::Result<Result<keyvalue::store::KeyResponse>> {
        propagate_trace_for_ctx!(context);
        Ok(self
            .list_keys(context, bucket, cursor.unwrap_or_default())
            .await)
    }
}

/// Handle provider control commands, the minimum required of any provider on
/// a wasmcloud lattice
impl Provider for KvVaultProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-component resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            source_id,
            link_name,
            config,
            ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        debug!(
           %source_id,
           %link_name,
            "adding link for component",
        );

        let config = match Config::from_values(config) {
            Ok(config) => config,
            Err(e) => {
                error!(
                    %source_id,
                    %link_name,
                    "failed to parse config: {e}",
                );
                bail!(anyhow!(e).context("failed to parse config"))
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
                return Err(anyhow!(e).context("failed to create new client config"));
            }
        };
        client.set_renewal().await;

        let mut update_map = self.components.write().await;
        update_map.insert(source_id.to_string(), Arc::new(client));

        Ok(())
    }

    /// Handle notification that a link is dropped - close the connection
    #[instrument(level = "debug", skip(self))]
    async fn delete_link(&self, source_id: &str) -> anyhow::Result<()> {
        let mut aw = self.components.write().await;
        if let Some(client) = aw.remove(source_id) {
            debug!("deleting link for component [{source_id}]");
            drop(client);
        }
        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> anyhow::Result<()> {
        let mut aw = self.components.write().await;
        // Empty the component link data and stop all servers
        for (_, client) in aw.drain() {
            drop(client);
        }
        Ok(())
    }
}
