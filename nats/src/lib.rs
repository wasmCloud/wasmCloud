#[macro_use]
extern crate wascc_codec as codec;

extern crate wasmcloud_actor_core as actorcore;
extern crate wasmcloud_actor_messaging as messaging;

mod natsprov;

#[allow(unused)] // used by the Makefile
const CAPABILITY_ID: &str = "wasmcloud:messaging";

#[macro_use]
extern crate log;

use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};

pub const OP_DELIVER_MESSAGE: &str = "DeliverMessage";
pub const OP_PUBLISH_MESSAGE: &str = "Publish";
pub const OP_PERFORM_REQUEST: &str = "Request";

use codec::core::{OP_BIND_ACTOR, OP_HEALTH_REQUEST, OP_REMOVE_ACTOR};
use messaging::{BrokerMessage, RequestArgs};

use actorcore::{CapabilityConfiguration, HealthCheckResponse};
use std::collections::HashMap;
use wascc_codec::{deserialize, serialize};

use std::error::Error;
use std::sync::Arc;
use std::sync::RwLock;

#[cfg(not(feature = "static_plugin"))]
capability_provider!(NatsProvider, NatsProvider::new);

/// NATS implementation of the `wasmcloud:messaging` specification
#[derive(Clone)]
pub struct NatsProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    clients: Arc<RwLock<HashMap<String, nats::Connection>>>,
}

impl Default for NatsProvider {
    fn default() -> Self {
        match env_logger::try_init() {
            Ok(_) => {}
            Err(_) => {}
        };

        NatsProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            clients: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl NatsProvider {
    /// Creates a new NATS provider. This is either invoked manually in static plugin
    /// mode, or invoked by the host during dynamic loading
    pub fn new() -> NatsProvider {
        Self::default()
    }

    fn publish_message(
        &self,
        actor: &str,
        msg: BrokerMessage,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let lock = self.clients.read().unwrap();
        let client = lock.get(actor).unwrap();

        natsprov::publish(&client, msg)
    }

    fn request(
        &self,
        actor: &str,
        msg: RequestArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let lock = self.clients.read().unwrap();
        let client = lock.get(actor).unwrap();

        natsprov::request(&client, msg)
    }

    fn configure(
        &self,
        msg: CapabilityConfiguration,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        if self.clients.read().unwrap().contains_key(&msg.module) {
            return Ok(vec![]);
        }
        let d = self.dispatcher.clone();
        let c = natsprov::initialize_client(d, &msg.module, &msg.values)?;

        self.clients.write().unwrap().insert(msg.module, c);
        Ok(vec![])
    }

    fn remove_actor(
        &self,
        msg: CapabilityConfiguration,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        info!("Removing NATS client for actor {}", msg.module);
        self.clients.write().unwrap().remove(&msg.module);
        Ok(vec![])
    }
}

impl CapabilityProvider for NatsProvider {
    /// Receives a dispatcher from the host runtime
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn Dispatcher>,
    ) -> Result<(), Box<dyn Error + Sync + Send>> {
        trace!("Dispatcher received.");
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    /// Handles an invocation received from the host runtime
    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        trace!("Received host call from {}, operation - {}", actor, op);

        match op {
            OP_PUBLISH_MESSAGE => self.publish_message(actor, deserialize(msg)?),
            OP_PERFORM_REQUEST => self.request(actor, deserialize(msg)?),
            OP_BIND_ACTOR if actor == "system" => self.configure(deserialize(msg)?),
            OP_REMOVE_ACTOR if actor == "system" => self.remove_actor(deserialize(msg)?),
            OP_HEALTH_REQUEST if actor == "system" => Ok(serialize(HealthCheckResponse {
                healthy: true,
                message: "".to_string(),
            })
            .unwrap()),
            _ => Err("bad dispatch".into()),
        }
    }

    fn stop(&self) {}
}
