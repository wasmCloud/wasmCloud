use std::collections::HashMap;

use anyhow::Context as _;
use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument};

use wasmcloud_provider_wit_bindgen::deps::{
    async_trait::async_trait, serde_json, serde_json::Value,
    wasmcloud_provider_sdk::core::LinkDefinition, wasmcloud_provider_sdk::Context,
};

pub(crate) mod client;
pub(crate) mod config;
pub(crate) mod error;

use crate::client::Client;
use crate::config::Config;
use crate::error::VaultError;

/// Token to indicate string data was passed during set
pub const STRING_VALUE_MARKER: &str = "string_data___";

wasmcloud_provider_wit_bindgen::generate!({
    impl_struct: KvVaultProvider,
    contract: "wasmcloud:keyvalue",
    wit_bindgen_cfg: "provider-kv-vault"
});

/// Redis KV provider implementation which utilizes [Hashicorp Vault](https://developer.hashicorp.com/vault/docs)
#[derive(Default, Clone)]
pub struct KvVaultProvider {
    // store redis connections per actor
    actors: std::sync::Arc<RwLock<HashMap<String, RwLock<Client>>>>,
}

impl KvVaultProvider {
    /// Retrieve a client for a given context (determined by actor_id)
    async fn get_client(&self, ctx: &Context) -> Result<Client> {
        // get the actor ID
        let actor_id = ctx
            .actor
            .as_ref()
            .context("invalid parameter: no actor in request")?;

        // Clone the existing client for the given actor from the internal hash map
        let client = self
            .actors
            .read()
            .await
            .get(actor_id)
            .with_context(|| format!("invalid parameter: actor [{actor_id}] not linked"))?
            .read()
            .await
            .clone();
        Ok(client)
    }
}

/// Handle provider control commands, the minimum required of any provider on
/// a wasmcloud lattice
#[async_trait]
impl WasmcloudCapabilityProvider for KvVaultProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip(self, ld), fields(actor_id = %ld.actor_id))]
    async fn put_link(&self, ld: &LinkDefinition) -> bool {
        let config = match Config::from_values(&HashMap::from_iter(ld.values.clone().into_iter())) {
            Ok(config) => config,
            Err(e) => {
                error!(
                    actor_id = %ld.actor_id,
                    link_name = %ld.link_name,
                    "failed to parse config: {e}",
                );
                return false;
            }
        };

        let client = match Client::new(config.clone()) {
            Ok(client) => client,
            Err(e) => {
                error!(
                    actor_id = %ld.actor_id,
                    link_name = %ld.link_name,
                    "failed to create new client config: {e}",
                );
                return false;
            }
        };

        let mut update_map = self.actors.write().await;
        info!(
            actor_id = %ld.actor_id,
            link_name = %ld.link_name,
            "adding link for actor",
        );
        update_map.insert(ld.actor_id.to_string(), RwLock::new(client));
        true
    }

    /// Handle notification that a link is dropped - close the connection
    #[instrument(level = "debug", skip(self))]
    async fn delete_link(&self, actor_id: &str) {
        let mut aw = self.actors.write().await;
        if let Some(client) = aw.remove(actor_id) {
            info!("deleting link for actor [{actor_id}]");
            drop(client)
        }
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) {
        let mut aw = self.actors.write().await;
        // Empty the actor link data and stop all servers
        for (_, client) in aw.drain() {
            drop(client)
        }
    }
}

/// Handle KeyValue methods that interact with redis
#[async_trait]
impl WasmcloudKeyvalueKeyValue for KvVaultProvider {
    /// Gets a value for a specified key. Deserialize the value as json
    /// if it's a map containing the key STRING_VALUE_MARKER, with a sting value, return the value
    /// If it's any other map, the entire map is returned as a serialized json string
    /// If the stored value is a plain string, returns the plain value
    /// All other values are returned as serialized json
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, arg = %arg.to_string()))]
    async fn get(&self, ctx: Context, arg: String) -> GetResponse {
        let client = match self.get_client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return GetResponse {
                    exists: false,
                    value: String::default(),
                };
            }
        };

        match client.read_secret::<Value>(&arg.to_string()).await {
            Ok(Value::Object(mut map)) => {
                if let Some(Value::String(value)) = map.remove(STRING_VALUE_MARKER) {
                    GetResponse {
                        value,
                        exists: true,
                    }
                } else {
                    GetResponse {
                        value: serde_json::to_string(&map).unwrap(),
                        exists: true,
                    }
                }
            }
            Ok(Value::String(value)) => GetResponse {
                value,
                exists: true,
            },
            Ok(value) => GetResponse {
                value: serde_json::to_string(&value).unwrap(),
                exists: true,
            },
            Err(VaultError::NotFound { namespace, path }) => {
                debug!(
                    %namespace, %path,
                    "vault read NotFound error"
                );
                GetResponse {
                    exists: false,
                    value: String::default(),
                }
            }
            Err(e) => {
                error!(error = %e, "vault read: other error");
                GetResponse {
                    exists: false,
                    value: String::default(),
                }
            }
        }
    }

    /// Returns true if the store contains the key
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, arg = %arg.to_string()))]
    async fn contains(&self, ctx: Context, arg: String) -> bool {
        matches!(
            self.get(ctx.clone(), arg.to_string()).await,
            GetResponse { exists: true, .. }
        )
    }

    /// Deletes a key, returning true if the key was deleted
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, arg = %arg.to_string()))]
    async fn del(&self, ctx: Context, arg: String) -> bool {
        let client = match self.get_client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return false;
            }
        };

        match client.delete_latest(&arg.to_string()).await {
            Ok(_) => true,
            Err(VaultError::NotFound { namespace, path }) => {
                debug!(%namespace, %path, "vault delete NotFound error");
                false
            }
            Err(e) => {
                debug!(error = %e, "Error while deleting from vault");
                false
            }
        }
    }

    /// Increments a numeric value, returning the new value
    async fn increment(&self, _ctx: Context, _arg: IncrementRequest) -> i32 {
        error!("`increment` not implemented");
        0
    }

    /// Append a value onto the end of a list. Returns the new list size
    async fn list_add(&self, _ctx: Context, _arg: ListAddRequest) -> u32 {
        error!("`list_add` not implemented");
        0
    }

    /// Deletes a list and its contents
    /// input: list name
    /// returns: true if the list existed and was deleted
    async fn list_clear(&self, _ctx: Context, _arg: String) -> bool {
        error!("`list_clear` not implemented");
        false
    }

    /// Deletes an item from a list. Returns true if the item was removed.
    async fn list_del(&self, _ctx: Context, _arg: ListDelRequest) -> bool {
        error!("`list_del` not implemented");
        false
    }

    /// Retrieves a range of values from a list using 0-based indices.
    /// Start and end values are inclusive, for example, (0,10) returns
    /// 11 items if the list contains at least 11 items. If the stop value
    /// is beyond the end of the list, it is treated as the end of the list.
    async fn list_range(&self, _ctx: Context, _arg: ListRangeRequest) -> Vec<String> {
        error!("`list_range` not implemented");
        Vec::new()
    }

    /// Sets the value of a key.
    /// expiration times are not supported by this api and should be 0.
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.key))]
    async fn set(&self, ctx: Context, arg: SetRequest) -> () {
        let client = match self.get_client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return;
            }
        };

        let value: Value = serde_json::from_str(&arg.value).unwrap_or_else(|_| {
            let mut map = serde_json::Map::new();
            map.insert(
                STRING_VALUE_MARKER.to_string(),
                Value::String(arg.value.clone()),
            );
            Value::Object(map)
        });
        match client.write_secret(&arg.key, &value).await {
            Ok(metadata) => {
                debug!(?metadata, "set returned metadata");
            }
            Err(VaultError::NotFound { namespace, path }) => {
                error!(
                    %namespace, %path,
                    "write secret returned not found, returning empty results",
                );
            }
            Err(e) => {
                error!(error = %e, "vault set: other error");
            }
        }
    }

    /// Add an item into a set. Returns number of items added
    async fn set_add(&self, _ctx: Context, _arg: SetAddRequest) -> u32 {
        error!("`set_add` not implemented");
        0
    }

    /// Remove a item from the set. Returns
    async fn set_del(&self, _ctx: Context, _arg: SetDelRequest) -> u32 {
        error!("`set_del` not implemented");
        0
    }

    async fn set_intersection(&self, _ctx: Context, _arg: Vec<String>) -> Vec<String> {
        error!("`set_intersection` not implemented");
        Vec::new()
    }

    /// returns a list of all secrets at the path
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, arg = %arg.to_string()))]
    async fn set_query(&self, ctx: Context, arg: String) -> Vec<String> {
        let client = match self.get_client(&ctx).await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to retrieve client: {e}");
                return Vec::new();
            }
        };

        match client.list_secrets(&arg.to_string()).await {
            Ok(list) => list,
            Err(VaultError::NotFound { namespace, path }) => {
                error!(
                    %namespace, %path,
                    "list secrets not found, returning empty results",
                );
                Vec::new()
            }
            Err(e) => {
                error!(error = %e, "vault list: other error");
                Vec::new()
            }
        }
    }

    async fn set_union(&self, _ctx: Context, _arg: Vec<String>) -> Vec<String> {
        error!("`set_union` not implemented");
        Vec::new()
    }

    /// Deletes a set and its contents
    /// input: set name
    /// returns: true if the set existed and was deleted
    async fn set_clear(&self, _ctx: Context, _arg: String) -> bool {
        error!("`set_clear` not implemented");
        false
    }
}
