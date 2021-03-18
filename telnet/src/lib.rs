#[macro_use]
extern crate wasmcloud_provider_core as codec;

use log::{info, trace};
mod server;
mod session;

use codec::{
    capabilities::{CapabilityProvider, Dispatcher, NullDispatcher},
    core::{OP_BIND_ACTOR, OP_HEALTH_REQUEST, OP_REMOVE_ACTOR, SYSTEM_ACTOR},
    deserialize, serialize,
};
use crossbeam::channel::Sender;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    error::Error,
    sync::{Arc, RwLock},
};
use wasmcloud_actor_core::{CapabilityConfiguration, HealthCheckResponse};

type MessageHandlerResult = Result<Vec<u8>, Box<dyn Error + Send + Sync + 'static>>;

#[allow(dead_code)]
const CAPABILITY_ID: &str = "wasmcloud:telnet";

pub(crate) const OP_SEND_TEXT: &str = "SendText";
pub(crate) const OP_SESSION_STARTED: &str = "SessionStarted";
pub(crate) const OP_RECEIVE_TEXT: &str = "ReceiveText";

#[cfg(not(feature = "static_plugin"))]
capability_provider!(TelnetProvider, TelnetProvider::new);

#[derive(Clone)]
pub struct TelnetProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    outbounds: Arc<RwLock<HashMap<String, Sender<String>>>>,
}

impl Default for TelnetProvider {
    fn default() -> Self {
        let _ = env_logger::try_init();

        TelnetProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            outbounds: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl TelnetProvider {
    pub fn new() -> Self {
        Self::default()
    }

    fn configure(&self, config: CapabilityConfiguration) -> MessageHandlerResult {
        session::start_server(
            std::fs::read_to_string(&config.values.get("MOTD").unwrap_or(&"".to_string()))?,
            config
                .values
                .get("PORT")
                .unwrap_or(&"3000".to_string())
                .parse()
                .unwrap(),
            &config.module,
            self.dispatcher.clone(),
            self.outbounds.clone(),
        );

        Ok(vec![])
    }

    fn deconfigure(&self, _config: CapabilityConfiguration) -> MessageHandlerResult {
        // Handle removal of resources claimed by an actor here
        // TODO: terminate the telnet server for this actor
        Ok(vec![])
    }

    /// Sends a text message to the appropriate socket
    fn send_text(&self, _actor: &str, msg: TelnetMessage) -> MessageHandlerResult {
        let outbound = self.outbounds.read().unwrap()[&msg.session].clone();
        outbound.send(msg.text).unwrap();
        Ok(vec![])
    }

    fn health(&self) -> MessageHandlerResult {
        Ok(serialize(HealthCheckResponse {
            healthy: true,
            message: "".to_string(),
        })?)
    }
}

impl CapabilityProvider for TelnetProvider {
    // Invoked by the runtime host to give this provider plugin the ability to communicate
    // with actors
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn Dispatcher>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Dispatcher received.");
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    // Invoked by host runtime to allow an actor to make use of the capability
    // All providers MUST handle the "configure" message, even if no work will be done
    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        trace!("Received host call from {}, operation - {}", actor, op);

        match op {
            OP_BIND_ACTOR if actor == SYSTEM_ACTOR => self.configure(deserialize(msg)?),
            OP_REMOVE_ACTOR if actor == SYSTEM_ACTOR => self.deconfigure(deserialize(msg)?),
            OP_HEALTH_REQUEST if actor == SYSTEM_ACTOR => self.health(),
            OP_SEND_TEXT => self.send_text(actor, deserialize(msg)?),
            _ => Err("bad dispatch".into()),
        }
    }

    fn stop(&self) {
        /* nothing to do */
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TelnetMessage {
    pub session: String,
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionStarted {
    pub session: String,
}
