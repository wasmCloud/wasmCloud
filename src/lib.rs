#[macro_use]
extern crate wascc_codec as codec;

#[macro_use]
extern crate log;

#[macro_use]
extern crate crossbeam;

#[macro_use]
extern crate serde_derive;

mod server;
mod session;

use codec::capabilities::{
    CapabilityDescriptor, CapabilityProvider, Dispatcher, NullDispatcher, OperationDirection,
    OP_GET_CAPABILITY_DESCRIPTOR,
};
use codec::core::{CapabilityConfiguration, OP_BIND_ACTOR, OP_REMOVE_ACTOR};
use codec::{deserialize, serialize};

use crossbeam::channel::Sender;
use std::error::Error;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

const SYSTEM_ACTOR: &str = "system";
const CAPABILITY_ID: &str = "wascc:telnet";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REVISION: u32 = 2; // Increment for each crates publish

pub(crate) const OP_SEND_TEXT: &str = "SendText";
pub(crate) const OP_SESSION_STARTED: &str = "SessionStarted";
pub(crate) const OP_RECEIVE_TEXT: &str = "ReceiveText";

#[cfg(not(feature = "static_plugin"))]
capability_provider!(TelnetProvider, TelnetProvider::new);

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

    fn configure(&self, config: CapabilityConfiguration) -> Result<Vec<u8>, Box<dyn Error>> {
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
        )?;

        Ok(vec![])
    }

    fn deconfigure(&self, _config: CapabilityConfiguration) -> Result<Vec<u8>, Box<dyn Error>> {
        // Handle removal of resources claimed by an actor here
        // TODO: terminate the telnet server for this actor
        Ok(vec![])
    }

    /// Sends a text message to the appropriate socket
    fn send_text(&self, _actor: &str, msg: TelnetMessage) -> Result<Vec<u8>, Box<dyn Error>> {
        let outbound = self.outbounds.read().unwrap()[&msg.session].clone();
        outbound.send(msg.text).unwrap();
        Ok(vec![])
    }

    fn get_descriptor(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        Ok(serialize(
            CapabilityDescriptor::builder()
                .id(CAPABILITY_ID)
                .name("waSCC Telnet Server Capability Provider")
                .long_description(
                    "A simple telnet server to allow waSCC actors to communicate via telnet",
                )
                .version(VERSION)
                .revision(REVISION)
                .with_operation(
                    OP_SESSION_STARTED,
                    OperationDirection::ToActor,
                    "Notifies an actor that a new telnet session has started",
                )
                .with_operation(
                    OP_SEND_TEXT,
                    OperationDirection::ToProvider,
                    "Sends text from an actor to a telnet session managed by the provider",
                )
                .with_operation(
                    OP_RECEIVE_TEXT,
                    OperationDirection::ToActor,
                    "Delivers a single line of text to an actor, tagged with the session ID",
                )
                .build(),
        )?)
    }
}

impl CapabilityProvider for TelnetProvider {
    // Invoked by the runtime host to give this provider plugin the ability to communicate
    // with actors
    fn configure_dispatch(&self, dispatcher: Box<dyn Dispatcher>) -> Result<(), Box<dyn Error>> {
        info!("Dispatcher received.");
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    // Invoked by host runtime to allow an actor to make use of the capability
    // All providers MUST handle the "configure" message, even if no work will be done
    fn handle_call(&self, actor: &str, op: &str, msg: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
        trace!("Received host call from {}, operation - {}", actor, op);

        match op {
            OP_BIND_ACTOR if actor == SYSTEM_ACTOR => self.configure(deserialize(msg)?),
            OP_REMOVE_ACTOR if actor == SYSTEM_ACTOR => self.deconfigure(deserialize(msg)?),
            OP_GET_CAPABILITY_DESCRIPTOR if actor == SYSTEM_ACTOR => self.get_descriptor(),
            OP_SEND_TEXT => self.send_text(actor, deserialize(msg)?),
            _ => Err("bad dispatch".into()),
        }
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
