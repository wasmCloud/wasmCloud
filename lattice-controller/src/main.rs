//! wasmCloud Lattice Control capability provider
//!
//!
use std::{collections::HashMap, convert::Infallible, sync::Arc, time::Duration};
use tokio::sync::RwLock;

use serde::{Deserialize, Serialize};
use wascap::prelude::KeyPair;
use wasmbus_rpc::anats;
use wasmbus_rpc::provider::prelude::*;
use wasmcloud_control_interface::*;

const DEFAULT_NATS_URI: &str = "127.0.0.1:4222";
const ENV_NATS_URI: &str = "URI";
const ENV_NATS_CLIENT_JWT: &str = "CLIENT_JWT";
const ENV_NATS_CLIENT_SEED: &str = "CLIENT_SEED";
const ENV_LATTICE_PREFIX: &str = "LATTICE_PREFIX";
const ENV_AUCTION_TIMEOUT_MS: &str = "AUCTION_TIMEOUT_MS";
const ENV_TIMEOUT_MS: &str = "TIMEOUT_MS";
const DEFAULT_TIMEOUT_MS: u64 = 2000;

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

impl ConnectionConfig {
    fn new_from(
        values: &HashMap<String, String>,
        mut nats_uris: Vec<String>,
    ) -> RpcResult<ConnectionConfig> {
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
            config.cluster_uris.append(&mut nats_uris);
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
    let hd = load_host_data()?;
    let mut lp = LatticeControllerProvider::default();

    // use the same nats for lattice control as we do for rpc,
    // unless it is overridden below by linkdefs
    let nats_addr = if !hd.lattice_rpc_url.is_empty() {
        hd.lattice_rpc_url.as_str()
    } else {
        DEFAULT_NATS_URI
    };

    if let Some(s) = hd.config_json.as_ref() {
        let mut hm = HashMap::new();
        hm.insert("config_b64".to_string(), s.to_owned());
        if let Ok(c) = ConnectionConfig::new_from(&hm, vec![nats_addr.to_string()]) {
            lp.fallback_config = c;
        }
    }

    provider_start(lp, hd)?;

    eprintln!("Lattice Controller capability provider exiting");
    Ok(())
}

/// lattice-controller capability provider implementation
#[derive(Default, Clone, Provider)]
#[services(LatticeController)]
struct LatticeControllerProvider {
    connections: Arc<RwLock<HashMap<String, Client>>>,
    fallback_config: ConnectionConfig,
}

impl LatticeControllerProvider {
    /// Create a nats connection and a Lattice controller client
    async fn create_client(&self, config: ConnectionConfig) -> RpcResult<Client> {
        let timeout = Duration::from_millis(config.timeout_ms);
        let auction_timeout = Duration::from_millis(config.auction_timeout_ms);
        let lattice_prefix = config.lattice_prefix.clone();
        let conn = connect(config).await?;
        let client = Client::new(conn, Some(lattice_prefix), timeout, auction_timeout);
        Ok(client)
    }

    async fn lookup_client(&self, ctx: &Context) -> RpcResult<Client> {
        let actor_id = ctx
            .actor
            .as_ref()
            .ok_or_else(|| RpcError::InvalidParameter("no actor in request".to_string()))?;
        let rd = self.connections.read().await;
        let client = rd
            .get(actor_id)
            .ok_or_else(|| RpcError::InvalidParameter(format!("actor not linked:{}", actor_id)))?;
        Ok(client.clone())
    }
}

/// Create a new nats connection
async fn connect(cfg: ConnectionConfig) -> RpcResult<anats::Connection> {
    let mut opts = match (cfg.auth_jwt, cfg.auth_seed) {
        (Some(jwt), Some(seed)) => {
            let kp = KeyPair::from_seed(&seed)
                .map_err(|e| RpcError::ProviderInit(format!("key init: {}", e)))?;

            anats::Options::with_jwt(
                move || Ok(jwt.clone()),
                move |nonce| kp.sign(nonce).unwrap(),
            )
        }
        (None, None) => anats::Options::default(),
        _ => {
            return Err(RpcError::InvalidParameter(
                "must provide both jwt and seed for jwt authentication".into(),
            ));
        }
    };
    opts = opts.with_name("wasmCloud Lattice Controller provider");
    let url = cfg.cluster_uris.get(0).unwrap();
    let conn = opts
        .connect(url)
        .await
        .map_err(|e| RpcError::ProviderInit(format!("Nats connection to {}: {}", url, e)))?;

    Ok(conn)
}

/// use default implementations of provider message handlers
impl ProviderDispatch for LatticeControllerProvider {}

#[async_trait]
impl ProviderHandler for LatticeControllerProvider {
    async fn put_link(&self, ld: &LinkDefinition) -> RpcResult<bool> {
        // Create one client (and nats connection) per actor_id.
        // This allows each actor's linkdef to have its own jwt credentials and/or nats urls.
        // Potential optimization not implemented:
        //   If multiple actors use the same credentials (or multiple actors use no credentials),
        //   a single nats connection could be shared by multiple actors ids,
        //   and the index in the HashMap could be hash(creds) instead of actor id.
        //   Since there aren't many actors linked to this provider, it's simpler
        //   to follow the pattern used by other capability providers: connections are indexed by actor id.
        if ld.values.contains_key("config_b64") || ld.values.contains_key("config_json") {
            let config =
                ConnectionConfig::new_from(&ld.values, self.fallback_config.cluster_uris.clone())?;
            let client = self.create_client(config).await?;
            let mut connections = self.connections.write().await;
            connections.insert(ld.actor_id.to_string(), client);
        }
        Ok(true)
    }

    /// Handle notification that a link is dropped
    async fn delete_link(&self, actor_id: &str) {
        let mut connections = self.connections.write().await;
        let _ = connections.remove(actor_id);
    }

    /// Handle shutdown request
    async fn shutdown(&self) -> Result<(), Infallible> {
        let mut connections = self.connections.write().await;
        connections.clear();

        Ok(())
    }
}

/// Handle LatticeController methods
#[async_trait]
impl LatticeController for LatticeControllerProvider {
    async fn set_registry_credentials(
        &self,
        ctx: &Context,
        arg: &RegistryCredentialMap,
    ) -> RpcResult<()> {
        self.lookup_client(ctx)
            .await?
            .put_registries(arg.clone())
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn auction_provider(
        &self,
        ctx: &Context,
        arg: &ProviderAuctionRequest,
    ) -> RpcResult<ProviderAuctionAcks> {
        self.lookup_client(ctx)
            .await?
            .perform_provider_auction(&arg.provider_ref, &arg.link_name, arg.constraints.clone())
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn auction_actor(
        &self,
        ctx: &Context,
        arg: &ActorAuctionRequest,
    ) -> RpcResult<ActorAuctionAcks> {
        self.lookup_client(ctx)
            .await?
            .perform_actor_auction(&arg.actor_ref, arg.constraints.clone())
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn get_hosts(&self, ctx: &Context) -> RpcResult<Hosts> {
        self.lookup_client(ctx)
            .await?
            .get_hosts()
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn get_host_inventory<TS: ToString + ?Sized + std::marker::Sync>(
        &self,
        ctx: &Context,
        arg: &TS,
    ) -> RpcResult<HostInventory> {
        self.lookup_client(ctx)
            .await?
            .get_host_inventory(&arg.to_string())
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn get_claims(&self, ctx: &Context) -> RpcResult<GetClaimsResponse> {
        self.lookup_client(ctx)
            .await?
            .get_claims()
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn start_actor(
        &self,
        ctx: &Context,
        arg: &StartActorCommand,
    ) -> RpcResult<CtlOperationAck> {
        self.lookup_client(ctx)
            .await?
            .start_actor(
                &arg.host_id,
                &arg.actor_ref,
                arg.count,
                arg.annotations.clone(),
            )
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn scale_actor(
        &self,
        ctx: &Context,
        arg: &ScaleActorCommand,
    ) -> RpcResult<CtlOperationAck> {
        self.lookup_client(ctx)
            .await?
            .scale_actor(
                &arg.host_id,
                &arg.actor_ref,
                &arg.actor_id,
                arg.count,
                arg.annotations.clone(),
            )
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn advertise_link(
        &self,
        ctx: &Context,
        arg: &wasmbus_rpc::core::LinkDefinition,
    ) -> RpcResult<CtlOperationAck> {
        self.lookup_client(ctx)
            .await?
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
        self.lookup_client(ctx)
            .await?
            .remove_link(&arg.actor_id, &arg.contract_id, &arg.link_name)
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn get_links(&self, ctx: &Context) -> RpcResult<LinkDefinitionList> {
        self.lookup_client(ctx)
            .await?
            .query_links()
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn update_actor(
        &self,
        ctx: &Context,
        arg: &UpdateActorCommand,
    ) -> RpcResult<CtlOperationAck> {
        self.lookup_client(ctx)
            .await?
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
        self.lookup_client(ctx)
            .await?
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
        self.lookup_client(ctx)
            .await?
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
        self.lookup_client(ctx)
            .await?
            .stop_actor(
                &arg.host_id,
                &arg.actor_ref,
                arg.count,
                arg.annotations.clone(),
            )
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }

    async fn stop_host(&self, ctx: &Context, arg: &StopHostCommand) -> RpcResult<CtlOperationAck> {
        self.lookup_client(ctx)
            .await?
            .stop_host(&arg.host_id, arg.timeout)
            .await
            .map_err(|e| RpcError::Nats(format!("{}", e)))
    }
}
