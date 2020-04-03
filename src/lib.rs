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

#[macro_use]
extern crate wascc_codec as codec;

use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use codec::core::{CapabilityConfiguration, OP_BIND_ACTOR, OP_REMOVE_ACTOR};
use codec::{
    deserialize,
    logging::{WriteLogRequest, OP_LOG},
};

#[macro_use]
extern crate log;

use std::error::Error;
use std::sync::RwLock;

#[cfg(not(feature = "static_plugin"))]
capability_provider!(LoggingProvider, LoggingProvider::new);

const CAPABILITY_ID: &str = "wascc:logging";
const SYSTEM_ACTOR: &str = "system";

const ERROR: u32 = 1;
const WARN: u32 = 2;
const INFO: u32 = 3;
const DEBUG: u32 = 4;
const TRACE: u32 = 5;

/// Standard output logging implementation of the `wascc:logging` specification
pub struct LoggingProvider {
    dispatcher: RwLock<Box<dyn Dispatcher>>,
}

impl Default for LoggingProvider {
    fn default() -> Self {
        match env_logger::try_init() {
            Ok(_) => {}
            Err(_) => {}
        }

        LoggingProvider {
            dispatcher: RwLock::new(Box::new(NullDispatcher::new())),
        }
    }
}

impl LoggingProvider {
    /// Creates a new logging provider
    pub fn new() -> Self {
        Self::default()
    }
}

impl CapabilityProvider for LoggingProvider {
    fn capability_id(&self) -> &'static str {
        CAPABILITY_ID
    }

    // Invoked by the runtime host to give this provider plugin the ability to communicate
    // with actors
    fn configure_dispatch(&self, dispatcher: Box<dyn Dispatcher>) -> Result<(), Box<dyn Error>> {
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    fn name(&self) -> &'static str {
        "waSCC Logging Provider"
    }

    // Invoked by host runtime to allow an actor to make use of the capability
    // All providers MUST handle the "configure" message, even if no work will be done
    fn handle_call(&self, actor: &str, op: &str, msg: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
        // TIP: do not allow individual modules to attempt to send configuration,
        // only accept it from the host runtime
        if op == OP_BIND_ACTOR && actor == SYSTEM_ACTOR {
            // if there were configuration values, we'd call
            // self.configure() here:
            //     self.configure(cfgvals).map(|_| vec![])

            Ok(vec![])
        } else if op == OP_REMOVE_ACTOR && actor == SYSTEM_ACTOR {
            let cfg_vals = deserialize::<CapabilityConfiguration>(msg)?;
            info!("Removing actor configuration for {}", cfg_vals.module);
            // tear down stuff here
            Ok(vec![])
        } else if op == OP_LOG {            
            let log_msg = deserialize::<WriteLogRequest>(msg)?;
            match log_msg.level {
                ERROR => error!("[{}] {}", actor, log_msg.body),
                WARN => warn!("[{}] {}", actor, log_msg.body),
                INFO => info!("[{}] {}", actor, log_msg.body),
                DEBUG => debug!("[{}] {}", actor, log_msg.body),
                TRACE => trace!("[{}] {}", actor, log_msg.body),
                _ => error!("Unknown log level: {}", log_msg.level),
            }
            Ok(vec![])
        } else {
            Err(format!("Unknown operation: {}", op).into())
        }
    }
}
