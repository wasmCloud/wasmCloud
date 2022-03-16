//! Hashicorp Vault implementation of the wasmcloud KeyValue capability contract wasmcloud:keyvalue
//!
use std::collections::HashMap;

use kv_vault_lib::{
    client::Client,
    config::Config,
    error::VaultError,
    wasmcloud_interface_keyvalue::{
        GetResponse, IncrementRequest, KeyValue, KeyValueReceiver, ListAddRequest, ListDelRequest,
        ListRangeRequest, SetAddRequest, SetDelRequest, SetRequest, StringList,
    },
    STRING_VALUE_MARKER,
};
use tokio::sync::RwLock;
use tracing::{debug, info, instrument};
use wasmbus_rpc::provider::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(atty::is(atty::Stream::Stderr))
        .init();
    // handle lattice control messages and forward rpc to the provider dispatch
    // returns when provider receives a shutdown control message
    provider_main(KvVaultProvider::default())?;

    eprintln!("KvVault provider exiting");
    Ok(())
}

/// Redis keyValue provider implementation.
#[derive(Default, Clone, Provider)]
#[services(KeyValue)]
struct KvVaultProvider {
    // store redis connections per actor
    actors: std::sync::Arc<RwLock<HashMap<String, RwLock<Client>>>>,
}
/// use default implementations of provider message handlers
impl ProviderDispatch for KvVaultProvider {}

/// Handle provider control commands
/// put_link (new actor link command), del_link (remove link command), and shutdown
#[async_trait]
impl ProviderHandler for KvVaultProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip(self, ld), fields(actor_id = %ld.actor_id))]
    async fn put_link(&self, ld: &LinkDefinition) -> RpcResult<bool> {
        let config = Config::from_values(&ld.values)?;
        let client = Client::new(config).map_err(to_rpc_err)?;
        let mut update_map = self.actors.write().await;
        info!("adding link for actor");
        update_map.insert(ld.actor_id.to_string(), RwLock::new(client));
        Ok(true)
    }

    /// Handle notification that a link is dropped - close the connection
    #[instrument(level = "debug", skip(self))]
    async fn delete_link(&self, actor_id: &str) {
        let mut aw = self.actors.write().await;
        if let Some(client) = aw.remove(actor_id) {
            info!("deleting link for actor");
            drop(client)
        }
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> Result<(), std::convert::Infallible> {
        let mut aw = self.actors.write().await;
        // empty the actor link data and stop all servers
        for (_, client) in aw.drain() {
            drop(client)
        }
        Ok(())
    }
}

fn to_rpc_err(e: VaultError) -> RpcError {
    RpcError::Other(format!("vault error: {}", e))
}

/// Handle KeyValue methods that interact with redis
#[async_trait]
impl KeyValue for KvVaultProvider {
    /// Increments a numeric value, returning the new value
    async fn increment(&self, _ctx: &Context, _arg: &IncrementRequest) -> RpcResult<i32> {
        Err(RpcError::NotImplemented)
    }

    /// Returns true if the store contains the key
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, arg = %arg.to_string()))]
    async fn contains<TS: ToString + ?Sized + Sync>(
        &self,
        ctx: &Context,
        arg: &TS,
    ) -> RpcResult<bool> {
        Ok(matches!(
            self.get(ctx, arg).await,
            Ok(GetResponse { exists: true, .. })
        ))
    }

    /// Deletes a key, returning true if the key was deleted
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, arg = %arg.to_string()))]
    async fn del<TS: ToString + ?Sized + Sync>(&self, ctx: &Context, arg: &TS) -> RpcResult<bool> {
        let client = self.get_client(ctx).await?;

        match client.delete_latest::<String>(&arg.to_string()).await {
            Ok(_) => Ok(true),
            Err(VaultError::NotFound { namespace, path }) => {
                debug!(%namespace, %path, "vault delete NotFound error");
                Ok(false)
            }
            Err(e) => {
                debug!(error = %e, "Error while deleting from vault");
                Err(to_rpc_err(e))
            }
        }
    }

    /// Gets a value for a specified key. Deserialize the value as json
    /// if it's a map containing the key STRING_VALUE_MARKER, with a sting value, return the value
    /// If it's any other map, the entire map is returned as a serialized json string
    /// If the stored value is a plain string, returns the plain value
    /// All other values are returned as serialized json
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, arg = %arg.to_string()))]
    async fn get<TS: ToString + ?Sized + Sync>(
        &self,
        ctx: &Context,
        arg: &TS,
    ) -> RpcResult<GetResponse> {
        use serde_json::Value;
        let client = self.get_client(ctx).await?;
        match client.read_secret::<Value>(&arg.to_string()).await {
            Ok(Value::Object(map)) => {
                if let Some(Value::String(value)) = map.get(STRING_VALUE_MARKER) {
                    Ok(GetResponse {
                        value: value.clone(),
                        exists: true,
                    })
                } else {
                    Ok(GetResponse {
                        value: serde_json::to_string(&map).unwrap(),
                        exists: true,
                    })
                }
            }
            Ok(Value::String(value)) => Ok(GetResponse {
                value,
                exists: true,
            }),
            Ok(value) => Ok(GetResponse {
                value: serde_json::to_string(&value).unwrap(),
                exists: true,
            }),
            Err(VaultError::NotFound { namespace, path }) => {
                debug!(
                    %namespace, %path,
                    "vault read NotFound error"
                );
                Ok(GetResponse {
                    exists: false,
                    ..Default::default()
                })
            }
            Err(e) => {
                debug!(error = %e, "vault read: other error");
                Err(to_rpc_err(e))
            }
        }
    }

    /// Append a value onto the end of a list. Returns the new list size
    async fn list_add(&self, _ctx: &Context, _arg: &ListAddRequest) -> RpcResult<u32> {
        Err(RpcError::NotImplemented)
    }

    /// Deletes a list and its contents
    /// input: list name
    /// returns: true if the list existed and was deleted
    async fn list_clear<TS: ToString + ?Sized + Sync>(
        &self,
        _ctx: &Context,
        _arg: &TS,
    ) -> RpcResult<bool> {
        Err(RpcError::NotImplemented)
    }

    /// Deletes an item from a list. Returns true if the item was removed.
    async fn list_del(&self, _ctx: &Context, _arg: &ListDelRequest) -> RpcResult<bool> {
        Err(RpcError::NotImplemented)
    }

    /// Retrieves a range of values from a list using 0-based indices.
    /// Start and end values are inclusive, for example, (0,10) returns
    /// 11 items if the list contains at least 11 items. If the stop value
    /// is beyond the end of the list, it is treated as the end of the list.
    async fn list_range(&self, _ctx: &Context, _arg: &ListRangeRequest) -> RpcResult<StringList> {
        Err(RpcError::NotImplemented)
    }

    /// Sets the value of a key.
    /// expiration times are not supported by this api and should be 0.
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, key = %arg.key))]
    async fn set(&self, ctx: &Context, arg: &SetRequest) -> RpcResult<()> {
        use serde_json::Value;
        let client = self.get_client(ctx).await?;
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
                debug!("set returned metadata: {:#?}", &metadata);
                Ok(())
            }
            Err(VaultError::NotFound { namespace, path }) => {
                debug!(
                    %namespace, %path,
                    "write secret returned not found, returning empty results",
                );
                Ok(())
            }
            Err(e) => {
                debug!(error = %e, "vault set: other error");
                Err(to_rpc_err(e))
            }
        }
    }

    /// Add an item into a set. Returns number of items added
    async fn set_add(&self, _ctx: &Context, _arg: &SetAddRequest) -> RpcResult<u32> {
        Err(RpcError::NotImplemented)
    }

    /// Remove a item from the set. Returns
    async fn set_del(&self, _ctx: &Context, _arg: &SetDelRequest) -> RpcResult<u32> {
        Err(RpcError::NotImplemented)
    }

    async fn set_intersection(
        &self,
        _ctx: &Context,
        _arg: &StringList,
    ) -> Result<StringList, RpcError> {
        Err(RpcError::NotImplemented)
    }

    /// returns a list of all secrets at the path
    #[instrument(level = "debug", skip(self, ctx, arg), fields(actor_id = ?ctx.actor, arg = %arg.to_string()))]
    async fn set_query<TS: ToString + ?Sized + Sync>(
        &self,
        ctx: &Context,
        arg: &TS,
    ) -> RpcResult<StringList> {
        let client = self.get_client(ctx).await?;
        match client.list_secrets(&arg.to_string()).await {
            Ok(list) => Ok(list),
            Err(VaultError::NotFound { namespace, path }) => {
                debug!(
                    %namespace, %path,
                    "list secrets not found, returning empty results",
                );
                Ok(Vec::new())
            }
            Err(e) => {
                debug!(error = %e, "vault list: other error");
                Err(to_rpc_err(e))
            }
        }
    }

    async fn set_union(&self, _ctx: &Context, _arg: &StringList) -> RpcResult<StringList> {
        Err(RpcError::NotImplemented)
    }

    /// Deletes a set and its contents
    /// input: set name
    /// returns: true if the set existed and was deleted
    async fn set_clear<TS: ToString + ?Sized + Sync>(
        &self,
        _ctx: &Context,
        _arg: &TS,
    ) -> RpcResult<bool> {
        Err(RpcError::NotImplemented)
    }
}

impl KvVaultProvider {
    /// Helper function to get client
    async fn get_client(&self, ctx: &Context) -> RpcResult<Client> {
        let actor_id = ctx
            .actor
            .as_ref()
            .ok_or_else(|| RpcError::InvalidParameter("no actor in request".to_string()))?;
        // get read lock on actor-client hashmap
        let rd = self.actors.read().await;
        let client_rw = rd
            .get(actor_id)
            .ok_or_else(|| RpcError::InvalidParameter(format!("actor not linked:{}", actor_id)))?;
        let x = Ok(client_rw.read().await.clone());
        x
    }
}
