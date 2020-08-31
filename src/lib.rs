// Copyright 2015-2020 Capital One Services, LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

mod nats;

#[macro_use]
extern crate wascc_codec as codec;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const REVISION: u32 = 2; // Increment for each crates publish

#[macro_use]
extern crate log;

use codec::capabilities::{
    CapabilityDescriptor, CapabilityProvider, Dispatcher, NullDispatcher, OperationDirection,
    OP_GET_CAPABILITY_DESCRIPTOR,
};
use codec::core::{OP_BIND_ACTOR, OP_REMOVE_ACTOR};
use codec::{
    messaging::{
        BrokerMessage, RequestMessage, OP_DELIVER_MESSAGE, OP_PERFORM_REQUEST, OP_PUBLISH_MESSAGE,
    },
    serialize,
};
use natsclient;
use std::collections::HashMap;
use wascc_codec::core::CapabilityConfiguration;
use wascc_codec::deserialize;

use std::error::Error;
use std::sync::Arc;
use std::sync::RwLock;

#[cfg(not(feature = "static_plugin"))]
capability_provider!(NatsProvider, NatsProvider::new);

const CAPABILITY_ID: &str = "wascc:messaging";

/// NATS implementation of the `wascc:messaging` specification
pub struct NatsProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    clients: Arc<RwLock<HashMap<String, natsclient::Client>>>,
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

        nats::publish(&client, msg)
    }

    fn request(
        &self,
        actor: &str,
        msg: RequestMessage,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let lock = self.clients.read().unwrap();
        let client = lock.get(actor).unwrap();

        nats::request(&client, msg)
    }

    fn configure(
        &self,
        msg: CapabilityConfiguration,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let d = self.dispatcher.clone();
        let c = nats::initialize_client(d, &msg.module, &msg.values)?;

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

    fn get_descriptor(&self) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        Ok(serialize(
            CapabilityDescriptor::builder()
                .id(CAPABILITY_ID)
                .name("Default waSCC Messaging Provider (NATS)")
                .long_description("A NATS-based implementation of the wascc:messaging contract")
                .version(VERSION)
                .revision(REVISION)
                .with_operation(
                    OP_PUBLISH_MESSAGE,
                    OperationDirection::ToProvider,
                    "Sends a message on a subject with an optional reply-to",
                )
                .with_operation(
                    OP_PERFORM_REQUEST,
                    OperationDirection::ToProvider,
                    "Sends a message on a subject expecting a reply on an auto-generated inbox",
                )
                .with_operation(
                    OP_DELIVER_MESSAGE,
                    OperationDirection::ToActor,
                    "Delivers a message from a NATS subscription to an actor",
                )
                .build(),
        )?)
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
            OP_GET_CAPABILITY_DESCRIPTOR if actor == "system" => self.get_descriptor(),
            OP_BIND_ACTOR if actor == "system" => self.configure(deserialize(msg)?),
            OP_REMOVE_ACTOR if actor == "system" => self.remove_actor(deserialize(msg)?),
            _ => Err("bad dispatch".into()),
        }
    }
}
