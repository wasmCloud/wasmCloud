#![cfg(not(target_arch = "wasm32"))]

//! common provider wasmbus support
//!
//!
//!
//
//   * wasmbus.rpc.{prefix}.{provider_key}.{link_name} - Get Invocation, answer InvocationResponse
//   * wasmbus.rpc.{prefix}.{public_key}.{link_name}.linkdefs.get - Query all link defs for this provider. (queue subscribed)
//   * wasmbus.rpc.{prefix}.{public_key}.{link_name}.linkdefs.del - Remove a link def. Provider de-provisions resources for the given actor.
//   * wasmbus.rpc.{prefix}.{public_key}.{link_name}.linkdefs.put - Puts a link def. Provider provisions resources for the given actor.
//   * wasmbus.rpc.{prefix}.{public_key}.{link_name}.shutdown - Request for graceful shutdown

use crate::{
    core::{CapabilityProvider, HealthCheckRequest, HealthCheckResponse, LinkDefinition},
    deserialize, serialize, Message, MessageDispatch, RpcError,
};
use anyhow::anyhow;
use nats::{Connection as NatsClient, Subscription};
use serde::{Deserialize, Serialize};
//use std::alloc::Global;
#[allow(unused_variables)]
use log::{debug, error, info, warn};
use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::oneshot;

pub type HostShutdownEvent = String;

pub trait ProviderDispatch: MessageDispatch + CapabilityProvider + ProviderHandler {}

pub mod prelude {
    pub use super::ProviderDispatch;
    pub use crate::core::CapabilityProvider;
    pub use crate::{client, context, Message, MessageDispatch, RpcError};

    //pub use crate::Timestamp;
    pub use async_trait::async_trait;
    pub use wasmbus_macros::Provider;

    #[cfg(feature = "BigInteger")]
    pub use num_bigint::BigInt as BigInteger;

    #[cfg(feature = "BigDecimal")]
    pub use bigdecimal::BigDecimal;
}

const HOST_DATA_ENV: &str = "WASMCLOUD_HOST_DATA";

pub fn load_host_data() -> Result<HostData, anyhow::Error> {
    let hd = std::env::var(HOST_DATA_ENV)
        .map_err(|_| anyhow!("env variable '{}' not found", HOST_DATA_ENV))?;
    let bytes = base64::decode(&hd)
        .map_err(|e| anyhow!("env variable '{}' is invalid base64: {}", HOST_DATA_ENV, e))?;
    let host_data: HostData = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow!("parsing '{}': {}", HOST_DATA_ENV, e))?;
    Ok(host_data)
}

/// The response to an invocation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HostData {
    pub host_id: String,
    pub lattice_rpc_prefix: String,
    pub link_name: String,
    pub lattice_rpc_user_jwt: String,
    pub lattice_rpc_user_seed: String,
    pub lattice_rpc_url: String,
    pub provider_key: String,
    #[serde(default)]
    pub env_values: HashMap<String, String>,
}

#[derive(Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WasmCloudEntity {
    pub public_key: String,
    pub link_name: String,
    pub contract_id: String,
}

#[derive(Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invocation {
    pub origin: WasmCloudEntity,
    pub target: WasmCloudEntity,
    pub operation: String,
    // I believe we determined this is necessary to properly round trip the "bytes"
    // type with Elixir so it doesn't treat it as a "list of u8s"
    #[serde(with = "serde_bytes")]
    pub msg: Vec<u8>,
    pub id: String,
    pub encoded_claims: String,
    pub host_id: String,
}

/// The response to an invocation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InvocationResponse {
    // I believe we determined this is necessary to properly round trip the "bytes"
    // type with Elixir so it doesn't treat it as a "list of u8s"
    #[serde(with = "serde_bytes")]
    pub msg: Vec<u8>,
    pub error: Option<String>,
    pub invocation_id: String,
}

/// CapabilityProvider handling of messages from host
/// The HostBridge handles most messages and forwards the remainder to this handler
pub trait ProviderHandler: Sync {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[allow(unused_variables)]
    fn put_link(&self, ld: &LinkDefinition) -> Result<bool, RpcError> {
        Ok(true)
    }

    /// Notify the provider that the link is dropped
    #[allow(unused_variables)]
    fn delete_link(&self, actor_id: &str) {}

    /// Handle system shutdown message
    fn shutdown(&self) -> Result<(), Infallible> {
        Ok(())
    }
}

pub struct HostBridge {
    pub subs: RwLock<Vec<Subscription>>,
    /// Table of actors that are bound to this provider
    /// Key is actor_id / actor public key
    pub links: RwLock<HashMap<String, LinkDefinition>>,
    pub nats: Option<NatsClient>,
}

impl Default for HostBridge {
    fn default() -> HostBridge {
        HostBridge {
            subs: RwLock::new(Vec::new()),
            links: RwLock::new(HashMap::new()),
            nats: None,
        }
    }
}

impl HostBridge {
    // Clear out all subscriptions
    pub fn unsubscribe_all(&self) {
        // make sure we call drain on all subscriptions
        for sub in self.subs.read().unwrap().iter() {
            let _ = sub.drain();
        }
    }

    /// returns copy of all links
    fn get_links(&self) -> Result<Vec<LinkDefinition>, RpcError> {
        let guard = self.links.read().unwrap();
        let links = guard.values().map(|ld| ld.clone()).collect();
        Ok(links)
    }

    /// Stores actor with link definition
    pub fn put_link(&self, ld: LinkDefinition) {
        self.links
            .write()
            .unwrap()
            .insert(ld.actor_id.to_string(), ld.clone());
    }

    /// Deletes link
    pub fn delete_link(&self, actor_id: &str) {
        self.links.write().unwrap().remove(actor_id);
    }

    /// Returns true if the actor is linked
    pub fn is_linked(&self, actor_id: &str) -> bool {
        self.links.read().unwrap().contains_key(actor_id)
    }

    /// Returns copy of LinkDefinition, or None,if the actor is not linked
    pub fn get_link(&self, actor_id: &str) -> Option<LinkDefinition> {
        self.links.read().unwrap().get(actor_id).cloned()
    }

    pub fn connect(
        &'static self,
        provider_key: String,
        link_name: &str,
        provider_impl: Arc<dyn ProviderDispatch + Send + Sync>, // + Send + Sync + 'static),
        shutdown_tx: oneshot::Sender<HostShutdownEvent>,
        _log_tx0: crossbeam::channel::Sender<LogEvent>,
    ) -> Result<(), anyhow::Error> {
        let nats = self.nats.as_ref().unwrap().clone();

        error!("++++++++++ connect main thread\n");
        // Link Get
        let ldget_topic = format!(
            "wasmbus.rpc.default.{}.{}.linkdefs.get",
            &provider_key, link_name
        );
        let sub = nats.queue_subscribe(&ldget_topic, &ldget_topic)?;
        //let pk = provider_key.to_string();
        self.subs.write().unwrap().push(sub.clone());
        let (this, subh) = (self.clone(), sub.clone());
        //let log_tx = log_tx0.clone();
        tokio::task::spawn(async move {
            for msg in subh.iter() {
                debug!("Received request for linkdefs.\n");
                let map = match this.get_links() {
                    Ok(links) => links
                        .into_iter()
                        .map(|ld| (ld.actor_id.to_string(), ld))
                        .collect::<HashMap<String, LinkDefinition>>(),
                    Err(e) => {
                        error!("get_links failed: {}\n", &e.to_string(),);
                        // return a dummy empty map
                        HashMap::new()
                    }
                };
                // It should be sufficient to return Vec<LinkDefinition>,
                // but the current api is defined to return a serialized map,
                // so generate that here
                let defs: Vec<u8> = match serialize(&map) {
                    Ok(defs) => defs,
                    Err(e) => {
                        error!("Error serializing link defs: {}", e);
                        Vec::new()
                    }
                };
                if let Err(e) = msg.respond(defs) {
                    error!("error respond to linkdefs: {}", e);
                }
            }
        });

        // Link Delete
        let lddel_topic = format!(
            "wasmbus.rpc.default.{}.{}.linkdefs.del",
            &provider_key, &link_name
        );
        let sub = nats.subscribe(&lddel_topic)?;
        self.subs.write().unwrap().push(sub.clone());
        let (this, subh, provider) = (self.clone(), sub.clone(), provider_impl.clone());
        //let log_tx = log_tx0.clone();
        tokio::task::spawn(async move {
            for msg in subh.iter() {
                match deserialize::<LinkDefinition>(&msg.data) {
                    Ok(ld) => {
                        this.delete_link(&ld.actor_id);
                        // notify provider that link is deleted
                        provider.delete_link(&ld.actor_id);
                    }
                    Err(e) => {
                        error!("error deserializing link-delete parameter: {}", e);
                    }
                }
            }
        });

        // Link Put
        let ldput_topic = format!(
            "wasmbus.rpc.default.{}.{}.linkdefs.put",
            &provider_key, &link_name
        );
        let sub = nats.subscribe(&ldput_topic)?;
        self.subs.write().unwrap().push(sub.clone());
        let (this, subh, provider) = (self.clone(), sub.clone(), provider_impl.clone());
        //let log_tx = log_tx0.clone();
        tokio::task::spawn(async move {
            for msg in subh.iter() {
                match deserialize::<LinkDefinition>(&msg.data) {
                    Ok(ld) => {
                        if this.is_linked(&ld.actor_id) {
                            // if alrady linked, print warning
                            warn!(
                                "Received LD put for existing link definition from {} to {}\n",
                                &ld.actor_id, &ld.provider_id
                            );
                        }
                        match provider.put_link(&ld) {
                            Ok(true) => {
                                info!("Linking {}\n", &ld.actor_id);
                                this.put_link(ld);
                            }
                            Ok(false) => {
                                // authorization failed or parameters were invalid
                                warn!("put_link denied: {}\n", &ld.actor_id);
                            }
                            Err(e) => {
                                error!("put_link {} failed: {}\n", &ld.actor_id, e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("error serializing put-link parameter: {}", e);
                    }
                }
            }
        });

        // Shutdown
        let shutdown_topic = format!(
            "wasmbus.rpc.default.{}.{}.shutdown",
            &provider_key, link_name
        );
        let sub = nats.subscribe(&shutdown_topic)?;
        // don't add this sub to subs list
        let (this, subh, provider) = (self.clone(), sub.clone(), provider_impl.clone());
        //let log_tx = log_tx0.clone();
        tokio::task::spawn(async move {
            for msg in subh.iter() {
                info!("Received termination signal. Shutting down capability provider.");
                // drain all subscriptions (other than this one)
                for sub in this.subs.read().unwrap().iter() {
                    let _ = sub.drain();
                }
                // tell provider to shutdown
                let _ = provider.shutdown();
                // send ack to host
                let _ = msg.respond("Shutting down");
                // signal main thread that we are ready
                if let Err(e) = shutdown_tx.send("bye".to_string()) {
                    error!("Problem shutting down:  failure to send signal: {}\n", e);
                }
                // exit this thread
                break;
            }
        });

        // Health check
        let health_topic = format!("wasmbus.rpc.{}.{}.health", &provider_key, &link_name);
        let sub = nats.subscribe(&health_topic)?;
        self.subs.write().unwrap().push(sub.clone());
        let (subh, provider) = (sub.clone(), provider_impl.clone());
        //let log_tx = log_tx0.clone();
        tokio::task::spawn(async move {
            let ctx = crate::context::Context::default();
            for msg in subh.iter() {
                debug!("received health check request");
                // placeholder arg
                let arg = HealthCheckRequest {};
                let resp = match provider.health_request(&ctx, &arg).await {
                    Ok(resp) => resp,
                    Err(e) => {
                        error!("error generating health check response: {}", &e);
                        HealthCheckResponse {
                            healthy: false,
                            message: Some(e.to_string()),
                        }
                    }
                };
                match serialize(&resp) {
                    Ok(t) => {
                        if let Err(e) = msg.respond(t) {
                            error!("failed sending health check response: {}", e.to_string());
                        }
                    }
                    Err(e) => {
                        // extremely unlikely that InvocationResponse would fail to serialize
                        error!("failed serializing HealthCheckResponse: {}", e.to_string());
                    }
                }
            }
        });

        // TODO: Add RPC handling for all app operations
        let rpc_topic = format!("wasmbus.rpc.default.{}.{}", &provider_key, link_name);
        let sub = nats.subscribe(&rpc_topic)?;
        self.subs.write().unwrap().push(sub.clone());
        let subh = sub.clone();
        //let log_tx = log_tx0.clone();
        tokio::task::spawn(async move {
            let provider = provider_impl.clone();
            let ctx = crate::context::Context::default();
            for msg in subh.iter() {
                debug!("received an RPC message");
                error!("++++++++++ RPC thread");
                let inv = match deserialize::<Invocation>(&msg.data) {
                    Ok(inv) => {
                        debug!(
                            "Received RPC Invocation: op:{} from:{}\n",
                            &inv.operation, &inv.origin.public_key,
                        );
                        inv
                    }
                    Err(e) => {
                        error!("received corrupt invocation: {}\n", e);
                        continue;
                    }
                };
                let provider_msg = Message {
                    method: &inv.operation,
                    arg: Cow::from(inv.msg),
                };
                let ir = match provider.dispatch(&ctx, provider_msg).await {
                    Ok(resp) => {
                        debug!("operation {} succeeded. returning response", &inv.operation);
                        InvocationResponse {
                            msg: resp.arg.to_vec(),
                            error: None,
                            invocation_id: inv.id,
                        }
                    }
                    Err(e) => {
                        error!(
                            "operation {} failed with error {}\n",
                            &inv.operation,
                            e.to_string()
                        );
                        InvocationResponse {
                            msg: Vec::new(),
                            error: Some(e.to_string()),
                            invocation_id: inv.id,
                        }
                    }
                };
                match serialize(&ir) {
                    Ok(t) => {
                        if let Err(e) = msg.respond(t) {
                            error!(
                                "failed sending response to op {}: {}",
                                &inv.operation,
                                e.to_string()
                            );
                        }
                    }
                    Err(e) => {
                        // extremely unlikely that InvocationResponse would fail to serialize
                        error!(
                            "failed serializing InvocationResponse to op:{} : {}\n",
                            &inv.operation,
                            e.to_string()
                        );
                    }
                }
            }
        });
        Ok(())
    }
}
