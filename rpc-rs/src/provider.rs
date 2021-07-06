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

use crate::{context, Message, MessageDispatch};
use crate::{deserialize, serialize};
use anyhow::anyhow;
use log::{debug, error, warn};
use nats::{Connection as NatsClient, Subscription};
use serde::{Deserialize, Serialize};
//use std::alloc::Global;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::oneshot;

pub type HostShutdownEvent = String;

pub mod prelude {
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
pub struct LinkDefinition {
    pub actor_id: String,
    pub provider_id: String,
    pub link_name: String,
    pub contract_id: String,
    pub values: HashMap<String, String>,
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

#[derive(Clone)]
pub struct HostNatsConnection {
    links: Arc<RwLock<HashMap<String, LinkDefinition>>>,
    subs: Arc<RwLock<Vec<Subscription>>>,
    nats: NatsClient,
}

impl Drop for HostNatsConnection {
    fn drop(&mut self) {
        // make sure we call drain on all subscriptions
        for sub in self.subs.read().unwrap().iter() {
            let _ = sub.drain();
        }
    }
}

// TODO:
//   - add hook for provider to say
//   - what kind of response should be sent if provider does not want to allow an actor to connect?

impl HostNatsConnection {
    pub fn new(nc: NatsClient) -> HostNatsConnection {
        HostNatsConnection {
            links: Arc::new(RwLock::new(HashMap::new())),
            subs: Arc::new(RwLock::new(Vec::new())),
            nats: nc,
        }
    }

    pub fn connect(
        &self,
        provider_key: &str,
        link_name: &str,
        //provider_impl: &'static (dyn MessageDispatch + Send + Sync + 'static),
        provider_impl: Box<dyn MessageDispatch + Send + Sync>, // + Send + Sync + 'static),
        tx: oneshot::Sender<HostShutdownEvent>,
    ) -> Result<(), anyhow::Error> {
        let ldget_topic = format!(
            "wasmbus.rpc.default.{}.{}.linkdefs.get",
            provider_key, link_name
        );
        let lddel_topic = format!(
            "wasmbus.rpc.default.{}.{}.linkdefs.del",
            provider_key, link_name
        );
        let ldput_topic = format!(
            "wasmbus.rpc.default.{}.{}.linkdefs.put",
            provider_key, link_name
        );
        let shutdown_topic = format!(
            "wasmbus.rpc.default.{}.{}.shutdown",
            provider_key, link_name
        );
        let rpc_topic = format!("wasmbus.rpc.default.{}.{}", provider_key, link_name);

        let sub = self.nats.queue_subscribe(&ldget_topic, &ldget_topic)?;
        let this = self.clone();
        self.subs.write().unwrap().push(sub.clone());
        tokio::task::spawn(async move {
            for msg in sub.iter() {
                debug!("Received request for linkdefs.");
                msg.respond(serialize(&*this.links.read().unwrap()).unwrap())
                    .unwrap();
            }
        });

        let sub = self.nats.subscribe(&lddel_topic)?;
        let this = self.clone();
        self.subs.write().unwrap().push(sub.clone());
        tokio::task::spawn(async move {
            for msg in sub.iter() {
                let ld: LinkDefinition = deserialize(&msg.data).unwrap();
                this.links.write().unwrap().remove(&ld.actor_id);
                debug!(
                    "Deleted link definition from {} to {}",
                    ld.actor_id, ld.provider_id
                );
                // TODO _del_ notification
            }
        });

        let sub = self.nats.subscribe(&ldput_topic)?;
        let this = self.clone();
        self.subs.write().unwrap().push(sub.clone());
        tokio::task::spawn(async move {
            for msg in sub.iter() {
                let ld: LinkDefinition = deserialize(&msg.data).unwrap();
                if this.links.read().unwrap().contains_key(&ld.actor_id) {
                    warn!(
                        "Received LD put for existing link definition from {} to {}",
                        ld.actor_id, ld.provider_id
                    );
                } else {
                    // TODO: _check_put_ notification
                    this.links
                        .write()
                        .unwrap()
                        .insert(ld.actor_id.to_string(), ld.clone());
                }

                // TODO: _put_ notification
                /*
                let conn = kvredis::initialize_client(ld.values.clone())
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                self.clients
                    .write()
                    .unwrap()
                    .insert(ld.actor_id.to_string(), conn);
                println!(
                    "Added link definition from {} to {}",
                    ld.actor_id, ld.provider_id
                );
                 */
            }
        });

        let sub = self.nats.subscribe(&shutdown_topic)?;
        let this = self.clone();
        // don't add this sub to subs list
        tokio::task::spawn(async move {
            for msg in sub.iter() {
                println!("Received termination signal. Shutting down capability provider.");
                // drail all subscriptions (other than this one)
                for sub in this.subs.read().unwrap().iter() {
                    let _ = sub.drain();
                }
                let _ = msg.respond("Shutting down");
                if let Err(e) = tx.send("bye".to_string()) {
                    error!("Problem shutting down:  failure to send signal: {}", e);
                }
                break;
            }
        });

        // TODO: Add RPC handling for all app operations
        let sub = self.nats.subscribe(&rpc_topic)?;
        //let this = self.clone();
        self.subs.write().unwrap().push(sub.clone());
        tokio::task::spawn(async move {
            for msg in sub.iter() {
                let inv = match deserialize::<Invocation>(&msg.data) {
                    Ok(inv) => {
                        debug!(
                            "Received RPC Invocation: op:{} from:{}",
                            &inv.operation, &inv.origin.public_key,
                        );
                        inv
                    }
                    Err(e) => {
                        error!("received corrupt invocation: {}", e);
                        continue;
                    }
                };
                let ctx = context::Context::default();
                let provider_msg = Message {
                    method: &inv.operation,
                    arg: Cow::from(inv.msg),
                };
                let ir = match provider_impl.dispatch(&ctx, provider_msg).await {
                    Ok(resp) => InvocationResponse {
                        msg: resp.arg.to_vec(),
                        error: None,
                        invocation_id: inv.id,
                    },
                    Err(e) => {
                        error!(
                            "operation {} failed with error {}",
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
                        let _ = msg.respond(t);
                    }
                    Err(e) => {
                        // extremely unlikely that InvocationResponse would fail to serialize
                        error!(
                            "failed serializing InvocationResponse to op:{} : {}",
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
