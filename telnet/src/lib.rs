use crossbeam_channel::Sender;
use log::{debug, info, warn};
use std::{
    collections::HashMap,
    error::Error,
    net::TcpListener,
    sync::{Arc, RwLock},
};
use wasmcloud_actor_core::{CapabilityConfiguration, HealthCheckResponse};
use wasmcloud_actor_telnet::{SendTextArgs, OP_SEND_TEXT};
use wasmcloud_provider_core::{
    capabilities::{CapabilityProvider, Dispatcher, NullDispatcher},
    capability_provider,
    core::{OP_BIND_ACTOR, OP_HEALTH_REQUEST, OP_REMOVE_ACTOR, SYSTEM_ACTOR},
    deserialize, serialize,
};
mod server;
mod session;

#[allow(dead_code)]
const CAPABILITY_ID: &str = "wasmcloud:telnet";

type MessageHandlerResult = Result<Vec<u8>, Box<dyn Error + Send + Sync + 'static>>;

#[cfg(not(feature = "static_plugin"))]
capability_provider!(TelnetProvider, TelnetProvider::new);

#[derive(Clone)]
pub struct TelnetProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    outbounds: Arc<RwLock<HashMap<String, Sender<String>>>>,
    listeners: Arc<RwLock<HashMap<String, TcpListener>>>,
}

impl Default for TelnetProvider {
    fn default() -> Self {
        let _ = env_logger::try_init();

        TelnetProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            outbounds: Arc::new(RwLock::new(HashMap::new())),
            listeners: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl TelnetProvider {
    pub fn new() -> Self {
        Self::default()
    }

    fn configure(&self, config: CapabilityConfiguration) -> MessageHandlerResult {
        if let Some(listener) = self.listeners.read().unwrap().get(&config.module) {
            debug!(
                "Telnet session for actor {} already listening on {}",
                listener.local_addr().unwrap(),
                &config.module
            );
            return Ok(vec![]);
        }

        session::start_server(
            config
                .values
                .get("MOTD")
                .map_or_else(|| "".to_string(), |motd| motd.to_string()),
            config
                .values
                .get("PORT")
                .map_or_else(|| Ok(3000), |p| p.parse())?,
            &config.module,
            self.dispatcher.clone(),
            self.outbounds.clone(),
            self.listeners.clone(),
        );
        Ok(vec![])
    }

    fn deconfigure(&self, config: CapabilityConfiguration) -> MessageHandlerResult {
        debug!("Shutting down telnet session for actor {}", &config.module);
        // Remove actor session, shutdown TCP listener
        if self
            .listeners
            .write()
            .unwrap()
            .remove(&config.module)
            .is_none()
        {
            warn!(
                "Attempted to deconfigure actor {}, but it was not configured",
                &config.module
            );
        }
        Ok(vec![])
    }

    /// Sends a text message to the appropriate socket
    fn send_text(
        &self,
        _actor: &str,
        msg: SendTextArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        match self.outbounds.read().unwrap().get(&msg.session).clone() {
            Some(outbound) => serialize(outbound.send(msg.text).is_ok()),
            None => Err(format!("Socket is not present for session {}", &msg.session).into()),
        }
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
        debug!("Received host call from {}, operation - {}", actor, op);

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
