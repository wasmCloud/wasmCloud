//! wasmCloud Lattice Control capability provider
//!
//!
use std::{collections::HashMap, convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::RwLock;

use log::debug;
use nats::asynk::{self, Connection};
use serde::{Deserialize, Serialize};
use wascap::prelude::KeyPair;
use wasmbus_rpc::provider::prelude::*;
use wasmcloud_control_interface::Client;
use wasmcloud_interface_lattice_control::*;

const DEFAULT_NATS_URI: &str = "0.0.0.0:4222";
const ENV_NATS_URI: &str = "URI";
const ENV_NATS_CLIENT_JWT: &str = "CLIENT_JWT";
const ENV_NATS_CLIENT_SEED: &str = "CLIENT_SEED";
const ENV_LATTICE_PREFIX: &str = "LATTICE_PREFIX";
const ENV_AUCTION_TIMEOUT_MS: &str = "AUCTION_TIMEOUT_MS";
const ENV_TIMEOUT_MS: &str = "TIMEOUT_MS";
const DEFAULT_TIMEOUT_MS: u64 = 2000;

/// Configuration for connecting a nats client.
/// More options are available if you use the json than variables in the values string map.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ConnectionConfig {
    #[serde(default)]
    cluster_uris: Vec<String>,
    #[serde(default)]
    auth_jwt: Option<String>,
    #[serde(default)]
    auth_seed: Option<String>,

    #[serde(default)]
    lattice_prefix: String,

    timeout_ms: u64,

    auction_timeout_ms: u64,
}

impl ConnectionConfig {
    fn new_from(values: &HashMap<String, String>) -> RpcResult<ConnectionConfig> {
        let mut config = if let Some(config_b64) = values.get("config_b64") {
            let bytes = base64::decode(config_b64.as_bytes()).map_err(|e| {
                RpcError::InvalidParameter(format!("invalid base64 encoding: {}", e))
            })?;
            serde_json::from_slice::<ConnectionConfig>(&bytes)
                .map_err(|e| RpcError::InvalidParameter(format!("corrupt config_b64: {}", e)))?
        } else if let Some(config) = values.get("config_json") {
            serde_json::from_str::<ConnectionConfig>(config)
                .map_err(|e| RpcError::InvalidParameter(format!("corrupt config_json: {}", e)))?
        } else {
            ConnectionConfig::default()
        };

        if let Some(url) = values.get(ENV_NATS_URI) {
            config.cluster_uris.push(url.clone());
        }
        if let Some(jwt) = values.get(ENV_NATS_CLIENT_JWT) {
            config.auth_jwt = Some(jwt.clone());
        }
        if let Some(seed) = values.get(ENV_NATS_CLIENT_SEED) {
            config.auth_seed = Some(seed.clone());
        }
        if config.auth_jwt.is_some() && config.auth_seed.is_none() {
            return Err(RpcError::InvalidParameter(
                "if you specify jwt, you must also specify a seed".to_string(),
            ));
        }
        if config.cluster_uris.is_empty() {
            config.cluster_uris.push(DEFAULT_NATS_URI.to_string());
        }

        if let Some(nsprefix) = values.get(ENV_LATTICE_PREFIX) {
            config.lattice_prefix = nsprefix.to_owned();
        }

        if let Some(auction_timeout) = values.get(ENV_AUCTION_TIMEOUT_MS) {
            config.auction_timeout_ms = auction_timeout.parse().unwrap_or(3 * DEFAULT_TIMEOUT_MS)
        }

        if let Some(timeout) = values.get(ENV_TIMEOUT_MS) {
            config.timeout_ms = timeout.parse().unwrap_or(DEFAULT_TIMEOUT_MS);
        }

        if config.timeout_ms == 0 {
            config.timeout_ms = DEFAULT_TIMEOUT_MS
        }

        if config.auction_timeout_ms == 0 {
            config.auction_timeout_ms = 3 * DEFAULT_TIMEOUT_MS
        }

        if config.lattice_prefix.is_empty() {
            config.lattice_prefix = "default".to_owned()
        }

        Ok(config)
    }
}

// main (via provider_main) initializes the threaded tokio executor,
// listens to lattice rpcs, handles actor links,
// and returns only when it receives a shutdown message
//
fn main() -> Result<(), Box<dyn std::error::Error>> {
    provider_main(LatticeControllerProvider::default())?;

    eprintln!("Lattice Controller capability provider exiting");
    Ok(())
}

/// lattice-controller capability provider implementation
#[derive(Default, Clone, Provider)]
#[services(LatticeController)]
struct LatticeControllerProvider {
    configs: Arc<RwLock<HashMap<String, ConnectionConfig>>>,
}

impl LatticeControllerProvider {
    async fn connect(&self, cfg: ConnectionConfig) -> Result<Connection, RpcError> {
        let mut opts = match (cfg.auth_jwt, cfg.auth_seed) {
            (Some(jwt), Some(seed)) => {
                let kp = KeyPair::from_seed(&seed)
                    .map_err(|e| RpcError::ProviderInit(format!("key init: {}", e)))?;
                asynk::Options::with_jwt(
                    move || Ok(jwt.clone()),
                    move |nonce| kp.sign(nonce).unwrap(),
                )
            }
            (None, None) => asynk::Options::new(),
            _ => {
                return Err(RpcError::InvalidParameter(
                    "must provide both jwt and seed for jwt authentication".into(),
                ));
            }
        };
        opts = opts.with_name("wasmCloud nats-messaging provider");
        let url = cfg.cluster_uris.get(0).unwrap();
        let conn = opts
            .connect(url)
            .await
            .map_err(|e| RpcError::ProviderInit(format!("Nats connection to {}: {}", url, e)))?;

        Ok(conn)
    }
}

/// use default implementations of provider message handlers
impl ProviderDispatch for LatticeControllerProvider {}
#[async_trait]
impl ProviderHandler for LatticeControllerProvider {
    async fn put_link(&self, ld: &LinkDefinition) -> RpcResult<bool> {
        let config = ConnectionConfig::new_from(&ld.values)?;

        let mut configs = self.configs.write().await;
        configs.insert(ld.actor_id.to_string(), config);

        Ok(true)
    }

    /// Handle notification that a link is dropped
    async fn delete_link(&self, actor_id: &str) {
        let mut configs = self.configs.write().await;
        let _ = configs.remove(actor_id);
    }

    /// Handle shutdown request
    async fn shutdown(&self) -> Result<(), Infallible> {
        let mut configs = self.configs.write().await;
        configs.clear();

        Ok(())
    }
}

fn get_actor(ctx: &Context) -> RpcResult<String> {
    ctx.actor
        .as_ref()
        .ok_or_else(|| RpcError::InvalidParameter("no actor in request".to_string()))
        .map(|s| s.to_string())
}

async fn get_config(lc: &LatticeControllerProvider, actor_id: &str) -> ConnectionConfig {
    lc.configs.read().await.get(actor_id).unwrap().clone()
}

// You might ask yourself, "Self, why would we want produce a new client upon every single
// request to the capability provider, rather than reuse existing ones?" That would be a good question!
// In environments subject to sporadic, unpredictable network partition events, it's much safer to bring up a new,
// clean connection, do work, and dispose rather than attempt to leave a persistent TCP connection
// running, which could become stale or "corrupt". Since we're not subscribing to anything,
// this is safer than maintaining a "live" connection per actor configured.
async fn create_client(
    ctx: &Context,
    lc: &LatticeControllerProvider,
) -> RpcResult<(Client, ConnectionConfig)> {
    let actor_id = get_actor(ctx)?;
    let config = get_config(lc, &actor_id).await;
    let conn = lc.connect(config.clone()).await?;
    let timeout = Duration::from_millis(config.timeout_ms);
    Ok((
        Client::new(
            conn.clone(),
            Some(config.lattice_prefix.to_owned()),
            timeout,
        ),
        config,
    ))
}

/// Handle LatticeController methods
#[async_trait]
impl LatticeController for LatticeControllerProvider {
    async fn auction_provider(
        &self,
        ctx: &Context,
        arg: &ProviderAuctionRequest,
    ) -> RpcResult<ProviderAuctionAcks> {
        let (client, config) = create_client(ctx, self).await?;
        let timeout = Duration::from_millis(config.auction_timeout_ms);
        client
            .perform_provider_auction(
                &arg.provider_ref,
                &arg.link_name,
                arg.constraints.clone(),
                timeout,
            )
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn auction_actor(
        &self,
        ctx: &Context,
        arg: &ActorAuctionRequest,
    ) -> RpcResult<ActorAuctionAcks> {
        let (client, config) = create_client(ctx, self).await?;
        let timeout = Duration::from_millis(config.auction_timeout_ms);

        client
            .perform_actor_auction(&arg.actor_ref, arg.constraints.clone(), timeout)
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn get_hosts(&self, ctx: &Context) -> RpcResult<Hosts> {
        let (client, config) = create_client(ctx, self).await?;
        let timeout = Duration::from_millis(config.auction_timeout_ms);

        client
            .get_hosts(timeout)
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn get_host_inventory<TS: ToString + ?Sized + std::marker::Sync>(
        &self,
        ctx: &Context,
        arg: &TS,
    ) -> RpcResult<HostInventory> {
        let (client, _config) = create_client(ctx, self).await?;
        client
            .get_host_inventory(&arg.to_string())
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn get_claims(&self, ctx: &Context) -> RpcResult<GetClaimsResponse> {
        let (client, _config) = create_client(ctx, self).await?;
        client
            .get_claims()
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn start_actor(
        &self,
        ctx: &Context,
        arg: &StartActorCommand,
    ) -> RpcResult<CtlOperationAck> {
        let (client, _config) = create_client(ctx, self).await?;
        client
            .start_actor(&arg.host_id, &arg.actor_ref, arg.annotations.clone())
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn advertise_link(
        &self,
        ctx: &Context,
        arg: &wasmbus_rpc::core::LinkDefinition,
    ) -> RpcResult<CtlOperationAck> {
        let (client, _config) = create_client(ctx, self).await?;
        client
            .advertise_link(
                &arg.actor_id,
                &arg.provider_id,
                &arg.contract_id,
                &arg.link_name,
                arg.values.clone(),
            )
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn remove_link(
        &self,
        ctx: &Context,
        arg: &RemoveLinkDefinitionRequest,
    ) -> RpcResult<CtlOperationAck> {
        let (client, _config) = create_client(ctx, self).await?;
        client
            .remove_link(&arg.actor_id, &arg.contract_id, &arg.link_name)
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn get_links(&self, ctx: &Context) -> RpcResult<LinkDefinitionList> {
        let (client, _config) = create_client(ctx, self).await?;
        client
            .query_links()
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn update_actor(
        &self,
        ctx: &Context,
        arg: &UpdateActorCommand,
    ) -> RpcResult<CtlOperationAck> {
        let (client, _config) = create_client(ctx, self).await?;
        client
            .update_actor(
                &arg.host_id,
                &arg.actor_id,
                &arg.new_actor_ref,
                arg.annotations.clone(),
            )
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn start_provider(
        &self,
        ctx: &Context,
        arg: &StartProviderCommand,
    ) -> RpcResult<CtlOperationAck> {
        let (client, _config) = create_client(ctx, self).await?;
        client
            .start_provider(
                &arg.host_id,
                &arg.provider_ref,
                Some(arg.link_name.to_owned()),
                arg.annotations.clone(),
                arg.configuration.clone(),
            )
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn stop_provider(
        &self,
        ctx: &Context,
        arg: &StopProviderCommand,
    ) -> RpcResult<CtlOperationAck> {
        let (client, _config) = create_client(ctx, self).await?;
        client
            .stop_provider(
                &arg.host_id,
                &arg.provider_ref,
                &arg.link_name,
                &arg.contract_id,
                arg.annotations.clone(),
            )
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn stop_actor(
        &self,
        ctx: &Context,
        arg: &StopActorCommand,
    ) -> RpcResult<CtlOperationAck> {
        let (client, _config) = create_client(ctx, self).await?;
        client
            .stop_actor(
                &arg.host_id,
                &arg.actor_ref,
                arg.count.unwrap_or(0),
                arg.annotations.clone(),
            )
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn stop_host(&self, ctx: &Context, arg: &StopHostCommand) -> RpcResult<CtlOperationAck> {
        let (client, _config) = create_client(ctx, self).await?;
        client
            .stop_host(&arg.host_id, arg.timeout)
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }
}
