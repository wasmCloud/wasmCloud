//! wasmCloud Lattice Control capability provider
//!
//!
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, RwLockReadGuard};
use tracing::{instrument, warn};
use wasmcloud_control_interface as interface_client;

mod client_cache;
use client_cache::ClientCache;

const DEFAULT_NATS_URI: &str = "0.0.0.0:4222";
const DEFAULT_TIMEOUT_MS: u64 = 2000;
// NOTE: Exercise caution when adjusting this value, as it can cause tests and
// other examples to fail, due to going against various timeouts set on the host and
// cooperating providers/actors, since the *entire* auction duration will be awaited
// for operations like `get-hosts`
const DEFAULT_AUCTION_TIMEOUT_MS: u64 = 3000;
const DEFAULT_LATTICE_PREFIX: &str = "default";

use wasmcloud_provider_sdk::error::{ProviderInvocationError, ProviderInvocationResult};
use wasmcloud_provider_sdk::Context;

wasmcloud_provider_wit_bindgen::generate!({
    impl_struct: LatticeControllerProvider,
    contract: "wasmcloud:latticecontrol",
    replace_witified_maps: true,
    wit_bindgen_cfg: "provider"
});

/// lattice-controller capability provider implementation
#[derive(Clone)]
pub struct LatticeControllerProvider {
    connection_timeout_mins: u64,
    connections: Arc<RwLock<Option<ClientCache>>>,
}

impl LatticeControllerProvider {
    /// Create a controller provider with a specified cache timeout in minutes
    pub fn with_cache_timeout_minutes(mins: u64) -> Self {
        Self {
            connection_timeout_mins: mins,
            connections: Arc::new(RwLock::new(None)),
        }
    }

    /// Retrieve the the connection cache, initializing if necessary
    ///
    /// This is necessary to avoid having to use an async `main()`,
    /// as the tokio runtime spawned will clash with an existing running one
    async fn get_connections(&self) -> RwLockReadGuard<ClientCache> {
        if self.connections.read().await.is_none() {
            let cache = ClientCache::new(self.connection_timeout_mins).await;
            let mut connections = self.connections.write().await;
            *connections = Some(cache);
            drop(connections);
        }
        let guard = self.connections.read().await;
        match RwLockReadGuard::try_map(guard, |v| v.as_ref()) {
            Ok(v) => v,
            Err(_) => unreachable!("the connections cache must have been initialized"),
        }
    }
}

/// Configuration for connecting a nats client.
/// More options are available if you use the json than variables in the values string map.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConnectionConfig {
    /// URIs used to connect to the cluster
    #[serde(default)]
    cluster_uris: Vec<String>,

    /// Authentication JWT
    #[serde(default)]
    auth_jwt: Option<String>,

    /// Authentication Seed
    #[serde(default)]
    auth_seed: Option<String>,

    /// Prefix for the lattice to use
    #[serde(default)]
    lattice_prefix: String,

    /// NATS JetStream domain
    #[serde(default)]
    js_domain: Option<String>,

    /// Operation timeout used for the lattice client interface
    timeout_ms: u64,

    /// Auction timeout used for the lattice client interface
    auction_timeout_ms: u64,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            cluster_uris: vec![DEFAULT_NATS_URI.to_owned()],
            auth_jwt: None,
            js_domain: None,
            auth_seed: None,
            lattice_prefix: String::from(DEFAULT_LATTICE_PREFIX),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            auction_timeout_ms: DEFAULT_AUCTION_TIMEOUT_MS,
        }
    }
}

/// Implement the basic requirements of a wasmcloud capability provider
#[async_trait]
impl WasmcloudCapabilityProvider for LatticeControllerProvider {
    #[instrument(level = "debug", skip(self, _ld), fields(actor_id = %_ld.actor_id))]
    async fn put_link(&self, _ld: &wasmcloud_provider_sdk::core::LinkDefinition) -> bool {
        // This provider is *externally* multiplexed -- link definitions
        // contain no useful data
        true
    }

    /// Handle notification that a link is dropped
    #[instrument(level = "debug", skip(self), fields(actor_id = ?_actor_id))]
    async fn delete_link(&self, _actor_id: &str) {
        // Link definitions do not determine the NATS connections
        // so there is nothing to clean up per link removal
    }

    /// Handle shutdown request
    #[instrument(level = "debug", skip(self))]
    async fn shutdown(&self) {
        // No cleanup necessary for shutdown, since links do not
        // determine/regulate the NATS connections
    }
}

/// Implement the lattice-controller-provider provider contract specified in WIT (see provider.wit)
#[async_trait]
impl WasmcloudLatticeControlLatticeController for LatticeControllerProvider {
    /// Sets lattice credentials and stores them in the cache to be used to create a
    /// connection in the next operation that requires one
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn set_lattice_credentials(
        &self,
        _ctx: Context,
        arg: SetLatticeCredentialsRequest,
    ) -> ProviderInvocationResult<CtlOperationAck> {
        let config = ConnectionConfig {
            cluster_uris: vec![arg
                .nats_url
                .clone()
                .unwrap_or_else(|| "0.0.0.0:4222".to_string())],
            auth_jwt: arg.user_jwt.clone(),
            auth_seed: arg.user_seed.clone(),
            lattice_prefix: arg.lattice_id.to_string(),
            js_domain: arg.js_domain.clone(),
            ..Default::default()
        };

        // Setting the auction_timeout to a large value may trigger timeouts in distant code.
        //
        // - host configuration rpc_timeout
        // - `LatticeControllerSender`s which have embedded RPC clients w/ internal timeouts
        // - httpserver/messaging provider(s) which expect a certain response speed from calling actors
        //
        // Since auctions will *wait* until the auction_timeout to do operations like gathering hosts,
        // we must manually ensure this value is unlikely to cause timeouts.
        let host_data = wasmcloud_provider_sdk::load_host_data().map_err(|e| {
            ProviderInvocationError::Provider(format!("failed to load host data: {e}"))
        })?;
        if host_data
            .default_rpc_timeout_ms
            .is_some_and(|v| v < config.timeout_ms)
        {
            warn!(
                host_rpc_timeout_ms = host_data.default_rpc_timeout_ms,
                auction_timeout = config.auction_timeout_ms,
                "host default RPC timeout < auction timeout, operations that rely on auctions are likely to time out"
            );
        }

        self.get_connections()
            .await
            .put_config(&arg.lattice_id, config)
            .await;

        Ok(CtlOperationAck {
            accepted: true,
            error: "".to_string(),
        })
    }

    /// Sets registry credentials, storing them in the cache
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn set_registry_credentials(
        &self,
        _ctx: Context,
        arg: SetRegistryCredentialsRequest,
    ) -> ProviderInvocationResult<()> {
        let client = self
            .get_connections()
            .await
            .get_client(&arg.lattice_id)
            .await?;
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
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?;
        Ok(())
    }

    /// Auction a provider on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn auction_provider(
        &self,
        _ctx: Context,
        arg: ProviderAuctionRequest,
    ) -> ProviderInvocationResult<Vec<ProviderAuctionAck>> {
        Ok(self
            .get_connections()
            .await
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
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Auction an actor on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = ?arg.lattice_id))]
    async fn auction_actor(
        &self,
        _ctx: Context,
        arg: ActorAuctionRequest,
    ) -> ProviderInvocationResult<Vec<ActorAuctionAck>> {
        Ok(self
            .get_connections()
            .await
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
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Retrieve all hosts on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = %arg.to_string()))]
    async fn get_hosts(&self, _ctx: Context, arg: String) -> ProviderInvocationResult<Vec<Host>> {
        Ok(self
            .get_connections()
            .await
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
                        prov_rpc_host: h.rpc_host.clone(),
                        rpc_host: h.rpc_host,
                        uptime_human: h.uptime_human,
                        uptime_seconds: h.uptime_seconds,
                        version: h.version,
                    })
                    .collect::<Vec<_>>()
            })
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Retrieve inventory for a given host on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, host = ?arg.host_id, lattice_id = ?arg.lattice_id))]
    async fn get_host_inventory(
        &self,
        _ctx: Context,
        arg: GetHostInventoryRequest,
    ) -> ProviderInvocationResult<HostInventory> {
        Ok(self
            .get_connections()
            .await
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
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Retrieve claims for a given client
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id))]
    async fn get_claims(
        &self,
        _ctx: Context,
        lattice_id: String,
    ) -> ProviderInvocationResult<GetClaimsResponse> {
        Ok(self
            .get_connections()
            .await
            .get_client(&lattice_id)
            .await?
            .get_claims()
            .await
            .map(|claims| GetClaimsResponse { claims })
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Start an actor on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = ?cmd.lattice_id))]
    async fn start_actor(
        &self,
        _ctx: Context,
        cmd: StartActorCommand,
    ) -> ProviderInvocationResult<CtlOperationAck> {
        Ok(self
            .get_connections()
            .await
            .get_client(&cmd.lattice_id.to_string())
            .await?
            .scale_actor(
                &cmd.host_id,
                &cmd.actor_ref,
                Some(cmd.count),
                Some(cmd.annotations.clone()),
            )
            .await
            .map(|ack| CtlOperationAck {
                accepted: ack.accepted,
                error: ack.error,
            })
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Scale an actor on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = ?cmd.lattice_id))]
    async fn scale_actor(
        &self,
        _ctx: Context,
        cmd: ScaleActorCommand,
    ) -> ProviderInvocationResult<CtlOperationAck> {
        Ok(self
            .get_connections()
            .await
            .get_client(&cmd.lattice_id.to_string())
            .await?
            .scale_actor(
                &cmd.host_id,
                &cmd.actor_ref,
                Some(cmd.count),
                Some(cmd.annotations.clone()),
            )
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Advertise a link on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = ?req.lattice_id))]
    async fn advertise_link(
        &self,
        _ctx: Context,
        req: AdvertiseLinkRequest,
    ) -> ProviderInvocationResult<CtlOperationAck> {
        Ok(self
            .get_connections()
            .await
            .get_client(&req.lattice_id.to_string())
            .await?
            .advertise_link(
                &req.link.actor_id,
                &req.link.provider_id,
                &req.link.contract_id,
                &req.link.link_name,
                req.link.values.clone().unwrap_or_default(),
            )
            .await
            .map(|ack| CtlOperationAck {
                accepted: ack.accepted,
                error: ack.error,
            })
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Remove a link on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = ?req.lattice_id))]
    async fn remove_link(
        &self,
        _ctx: Context,
        req: RemoveLinkDefinitionRequest,
    ) -> ProviderInvocationResult<CtlOperationAck> {
        Ok(self
            .get_connections()
            .await
            .get_client(&req.lattice_id.to_string())
            .await?
            .remove_link(&req.actor_id, &req.actor_id, &req.link_name)
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Retrieve links on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id))]
    async fn get_links(
        &self,
        _ctx: Context,
        lattice_id: String,
    ) -> ProviderInvocationResult<Vec<LinkDefinition>> {
        Ok(self
            .get_connections()
            .await
            .get_client(&lattice_id)
            .await?
            .query_links()
            .await
            .map(|links| {
                links
                    .into_iter()
                    .map(|client_ld| LinkDefinition {
                        actor_id: client_ld.actor_id,
                        provider_id: client_ld.provider_id,
                        contract_id: client_ld.contract_id,
                        link_name: client_ld.link_name,
                        values: Some(client_ld.values),
                    })
                    .collect()
            })
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Update an actor running on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = ?cmd.lattice_id))]
    async fn update_actor(
        &self,
        _ctx: Context,
        cmd: UpdateActorCommand,
    ) -> ProviderInvocationResult<CtlOperationAck> {
        Ok(self
            .get_connections()
            .await
            .get_client(&cmd.lattice_id.to_string())
            .await?
            .update_actor(
                &cmd.host_id,
                &cmd.actor_id,
                &cmd.new_actor_ref,
                cmd.annotations.clone(),
            )
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Start a provider on the lattice
    #[instrument(
        level = "debug", 
        skip_all, 
        fields(
            actor_id = ?_ctx.actor, 
            host_id = %cmd.host_id, 
            provider_ref = %cmd.provider_ref, 
            link_name = %cmd.link_name, 
            lattice_id = ?cmd.lattice_id
        )
    )]
    async fn start_provider(
        &self,
        _ctx: Context,
        cmd: StartProviderCommand,
    ) -> ProviderInvocationResult<CtlOperationAck> {
        Ok(self
            .get_connections()
            .await
            .get_client(&cmd.lattice_id.to_string())
            .await?
            .start_provider(
                &cmd.host_id,
                &cmd.provider_ref,
                Some(cmd.link_name.to_owned()),
                Some(cmd.annotations.clone()),
                Some(cmd.configuration.clone()),
            )
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Stop a provider on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = ?cmd.lattice_id))]
    async fn stop_provider(
        &self,
        _ctx: Context,
        cmd: StopProviderCommand,
    ) -> ProviderInvocationResult<CtlOperationAck> {
        Ok(self
            .get_connections()
            .await
            .get_client(&cmd.lattice_id.to_string())
            .await?
            .stop_provider(
                &cmd.host_id,
                &cmd.provider_id,
                &cmd.link_name,
                &cmd.contract_id,
                Some(cmd.annotations.clone()),
            )
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Stop an actor on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = ?cmd.lattice_id))]
    async fn stop_actor(
        &self,
        _ctx: Context,
        cmd: StopActorCommand,
    ) -> ProviderInvocationResult<CtlOperationAck> {
        Ok(self
            .get_connections()
            .await
            .get_client(&cmd.lattice_id.to_string())
            .await?
            .stop_actor(&cmd.host_id, &cmd.actor_id, Some(cmd.annotations.clone()))
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }

    /// Stop a host on the lattice
    #[instrument(level = "debug", skip_all, fields(actor_id = ?_ctx.actor, lattice_id = ?cmd.lattice_id))]
    async fn stop_host(
        &self,
        _ctx: Context,
        cmd: StopHostCommand,
    ) -> ProviderInvocationResult<CtlOperationAck> {
        Ok(self
            .get_connections()
            .await
            .get_client(&cmd.lattice_id.to_string())
            .await?
            .stop_host(&cmd.host_id, cmd.timeout)
            .await
            .map(|a| CtlOperationAck {
                accepted: a.accepted,
                error: a.error,
            })
            .map_err(|e| ProviderInvocationError::Provider(e.to_string()))?)
    }
}
