//! wasmCloud Lattice Control capability provider
//!
//!
use std::{collections::HashMap, convert::Infallible};

use client_cache::ClientCache;
use serde::{Deserialize, Serialize};
use tracing::instrument;
use wasmbus_rpc::provider::prelude::*;
use wasmcloud_control_interface as interface_client;
use wasmcloud_interface_lattice_control::*;

const DEFAULT_NATS_URI: &str = "0.0.0.0:4222";
const DEFAULT_TIMEOUT_MS: u64 = 2000;

mod client_cache;

/// lattice-controller capability provider implementation
#[derive(Clone, Provider)]
#[services(LatticeController)]
struct LatticeControllerProvider {
    connections: ClientCache,
}

/// Configuration for connecting a nats client.
/// More options are available if you use the json than variables in the values string map.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            cluster_uris: vec![DEFAULT_NATS_URI.to_owned()],
            auth_jwt: None,
            auth_seed: None,
            lattice_prefix: "default".to_owned(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            auction_timeout_ms: 3 * DEFAULT_TIMEOUT_MS,
        }
    }
}

// main (via provider_main) initializes the threaded tokio executor,
// listens to lattice rpcs, handles actor links,
// and returns only when it receives a shutdown message
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let hd = load_host_data()?;
    let lp = LatticeControllerProvider {
        connections: ClientCache::new(60 * client_cache::CACHE_EXPIRE_MINUTES).await,
    };

    provider_run(lp, hd, Some("Lattice Control Provider".to_string())).await?;

    eprintln!("Lattice Controller capability provider exiting");
    Ok(())
}

/// use default implementations of provider message handlers
impl ProviderDispatch for LatticeControllerProvider {}

#[async_trait]
impl ProviderHandler for LatticeControllerProvider {
    #[instrument(level = "debug", skip(self, _ld), fields(actor_id = %_ld.actor_id))]
    async fn put_link(&self, _ld: &LinkDefinition) -> RpcResult<bool> {
        // In this multiplexed version of the capability provider, link definitions
        // contain no data

        Ok(true)
    }

    /// Handle notification that a link is dropped
    #[instrument(level = "debug", skip(self), fields(actor_id = ?_actor_id))]
    async fn delete_link(&self, _actor_id: &str) {
        // Nothing extra is necessary here since link definitions are not
        // the unit of correlation to NATS connections
    }

    /// Handle shutdown request
    #[instrument(level = "debug", skip(self))]
    async fn shutdown(&self) -> Result<(), Infallible> {
        Ok(())
    }
}

/// Handle LatticeController methods
#[async_trait]
impl LatticeController for LatticeControllerProvider {
    /// Sets lattice credentials and stores them in the cache to be used to create a
    /// connection in the next operation that requires one
    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn set_lattice_credentials(
        &self,
        _ctx: &Context,
        arg: &SetLatticeCredentialsRequest,
    ) -> RpcResult<CtlOperationAck> {
        self.connections
            .put_config(
                &arg.lattice_id,
                ConnectionConfig {
                    cluster_uris: vec![arg
                        .nats_url
                        .clone()
                        .unwrap_or_else(|| "0.0.0.0:4222".to_string())],
                    auth_jwt: arg.user_jwt.clone(),
                    auth_seed: arg.user_seed.clone(),
                    lattice_prefix: arg.lattice_id.to_string(),
                    timeout_ms: 2_000,
                    auction_timeout_ms: 5_000,
                },
            )
            .await;

        Ok(CtlOperationAck {
            accepted: true,
            error: "".to_string(),
        })
    }

    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn set_registry_credentials(
        &self,
        _ctx: &Context,
        arg: &SetRegistryCredentialsRequest,
    ) -> RpcResult<()> {
        let client = self.connections.get_client(&arg.lattice_id).await?;
        let mut hm = HashMap::new();
        if let Some(ref c) = arg.credentials {
            for (k, v) in c {
                hm.insert(
                    k.to_string(),
                    interface_client::RegistryCredential {
                        password: v.password.clone(),
                        token: v.token.clone(),
                        username: v.username.clone(),
                        registry_type: "".to_string(),
                    },
                );
            }
        }

        client
            .put_registries(hm)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn auction_provider(
        &self,
        _ctx: &Context,
        arg: &ProviderAuctionRequest,
    ) -> RpcResult<ProviderAuctionAcks> {
        Ok(self
            .connections
            .get_client(&arg.lattice_id)
            .await?
            .perform_provider_auction(&arg.provider_ref, &arg.link_name, arg.constraints.clone())
            .await
            .map(|v| {
                v.into_iter()
                    .map(|a| ProviderAuctionAck {
                        host_id: a.host_id,
                        link_name: a.link_name,
                        provider_ref: a.provider_ref,
                    })
                    .collect::<Vec<_>>()
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn auction_actor(
        &self,
        _ctx: &Context,
        arg: &ActorAuctionRequest,
    ) -> RpcResult<ActorAuctionAcks> {
        Ok(self
            .connections
            .get_client(&arg.lattice_id)
            .await?
            .perform_actor_auction(&arg.actor_ref, arg.constraints.clone())
            .await
            .map(|v| {
                v.into_iter()
                    .map(|a| ActorAuctionAck {
                        actor_ref: a.actor_ref,
                        host_id: a.host_id,
                    })
                    .collect::<Vec<_>>()
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, lattice_id = %arg.to_string()))]
    async fn get_hosts<TS: ToString + ?Sized + std::marker::Sync>(
        &self,
        _ctx: &Context,
        arg: &TS,
    ) -> RpcResult<Hosts> {
        Ok(self
            .connections
            .get_client(&arg.to_string())
            .await?
            .get_hosts()
            .await
            .map(|v| {
                v.into_iter()
                    .map(|h| Host {
                        cluster_issuers: h.cluster_issuers,
                        ctl_host: h.ctl_host,
                        id: h.id,
                        js_domain: h.js_domain,
                        labels: h.labels,
                        lattice_prefix: h.lattice_prefix,
                        prov_rpc_host: h.prov_rpc_host,
                        rpc_host: h.rpc_host,
                        uptime_human: h.uptime_human,
                        uptime_seconds: h.uptime_seconds,
                        version: h.version,
                    })
                    .collect::<Vec<_>>()
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    // humble apologies for the jagged shape of the code below, but this is the price
    // we pay for not tightly coupling the consumer of this API to to the API
    // of the control client struct. This will continue to pay dividends if/when
    // the provider contract changes yet the lattice control client API does not.
    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, host = ?arg.host_id, lattice_id = ?arg.lattice_id))]
    async fn get_host_inventory(
        &self,
        _ctx: &Context,
        arg: &GetHostInventoryRequest,
    ) -> RpcResult<HostInventory> {
        Ok(self
            .connections
            .get_client(&arg.lattice_id.to_string())
            .await?
            .get_host_inventory(&arg.host_id)
            .await
            .map(|hi| HostInventory {
                actors: hi
                    .actors
                    .into_iter()
                    .map(|ad| ActorDescription {
                        id: ad.id,
                        image_ref: ad.image_ref,
                        instances: ad
                            .instances
                            .into_iter()
                            .map(|ai| ActorInstance {
                                annotations: ai.annotations,
                                instance_id: ai.instance_id,
                                revision: ai.revision,
                            })
                            .collect::<Vec<_>>(),
                        name: ad.name,
                    })
                    .collect::<Vec<_>>(),
                host_id: hi.host_id,
                labels: hi.labels,
                providers: hi
                    .providers
                    .into_iter()
                    .map(|pd| ProviderDescription {
                        annotations: pd.annotations,
                        id: pd.id,
                        image_ref: pd.image_ref,
                        link_name: pd.link_name,
                        name: pd.name,
                        revision: pd.revision,
                    })
                    .collect::<Vec<_>>(),
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, lattice_id = %arg.to_string()))]
    async fn get_claims<TS: ToString + ?Sized + std::marker::Sync>(
        &self,
        _ctx: &Context,
        arg: &TS,
    ) -> RpcResult<GetClaimsResponse> {
        Ok(self
            .connections
            .get_client(&arg.to_string())
            .await?
            .get_claims()
            .await
            .map(|c| GetClaimsResponse { claims: c.claims })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn start_actor(
        &self,
        _ctx: &Context,
        arg: &StartActorCommand,
    ) -> RpcResult<CtlOperationAck> {
        Ok(self
            .connections
            .get_client(&arg.lattice_id.to_string())
            .await?
            .start_actor(
                &arg.host_id,
                &arg.actor_ref,
                arg.count,
                arg.annotations.clone(),
            )
            .await
            .map(|ack| CtlOperationAck {
                accepted: ack.accepted,
                error: ack.error,
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn scale_actor(
        &self,
        _ctx: &Context,
        arg: &ScaleActorCommand,
    ) -> RpcResult<CtlOperationAck> {
        Ok(self
            .connections
            .get_client(&arg.lattice_id.to_string())
            .await?
            .scale_actor(
                &arg.host_id,
                &arg.actor_ref,
                &arg.actor_id,
                arg.count,
                arg.annotations.clone(),
            )
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn advertise_link(
        &self,
        _ctx: &Context,
        arg: &AdvertiseLinkRequest,
    ) -> RpcResult<CtlOperationAck> {
        Ok(self
            .connections
            .get_client(&arg.lattice_id.to_string())
            .await?
            .advertise_link(
                &arg.link.actor_id,
                &arg.link.provider_id,
                &arg.link.contract_id,
                &arg.link.link_name,
                arg.link.values.clone(),
            )
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn remove_link(
        &self,
        _ctx: &Context,
        arg: &RemoveLinkDefinitionRequest,
    ) -> RpcResult<CtlOperationAck> {
        Ok(self
            .connections
            .get_client(&arg.lattice_id.to_string())
            .await?
            .remove_link(&arg.actor_id, &arg.actor_id, &arg.link_name)
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, lattice_id = %arg.to_string()))]
    async fn get_links<TS: ToString + ?Sized + std::marker::Sync>(
        &self,
        _ctx: &Context,
        arg: &TS,
    ) -> RpcResult<LinkDefinitionList> {
        Ok(self
            .connections
            .get_client(&arg.to_string())
            .await?
            .query_links()
            .await
            .map(|res| LinkDefinitionList { links: res.links })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn update_actor(
        &self,
        _ctx: &Context,
        arg: &UpdateActorCommand,
    ) -> RpcResult<CtlOperationAck> {
        Ok(self
            .connections
            .get_client(&arg.lattice_id.to_string())
            .await?
            .update_actor(
                &arg.host_id,
                &arg.actor_id,
                &arg.new_actor_ref,
                arg.annotations.clone(),
            )
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx, arg), fields(actor_id = ?_ctx.actor, host_id = %arg.host_id, provider_ref = %arg.provider_ref, link_name = %arg.link_name, lattice_id = ?arg.lattice_id))]
    async fn start_provider(
        &self,
        _ctx: &Context,
        arg: &StartProviderCommand,
    ) -> RpcResult<CtlOperationAck> {
        Ok(self
            .connections
            .get_client(&arg.lattice_id.to_string())
            .await?
            .start_provider(
                &arg.host_id,
                &arg.provider_ref,
                Some(arg.link_name.to_owned()),
                arg.annotations.clone(),
                arg.configuration.clone(),
            )
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn stop_provider(
        &self,
        _ctx: &Context,
        arg: &StopProviderCommand,
    ) -> RpcResult<CtlOperationAck> {
        Ok(self
            .connections
            .get_client(&arg.lattice_id.to_string())
            .await?
            .stop_provider(
                &arg.host_id,
                &arg.provider_id,
                &arg.link_name,
                &arg.contract_id,
                arg.annotations.clone(),
            )
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn stop_actor(
        &self,
        _ctx: &Context,
        arg: &StopActorCommand,
    ) -> RpcResult<CtlOperationAck> {
        Ok(self
            .connections
            .get_client(&arg.lattice_id.to_string())
            .await?
            .stop_actor(
                &arg.host_id,
                &arg.actor_id,
                arg.count,
                arg.annotations.clone(),
            )
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }

    #[instrument(level = "debug", skip(self, _ctx), fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn stop_host(&self, _ctx: &Context, arg: &StopHostCommand) -> RpcResult<CtlOperationAck> {
        Ok(self
            .connections
            .get_client(&arg.lattice_id.to_string())
            .await?
            .stop_host(&arg.host_id, arg.timeout)
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| RpcError::Nats(e.to_string()))?)
    }
}
