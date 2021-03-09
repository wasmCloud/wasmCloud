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

use log::{debug, error, info, trace, warn};

use std::error::Error;
use std::sync::{Arc, RwLock};
use wasmcloud_actor_logging::{WriteLogArgs, OP_LOG};
use wasmcloud_provider_core::{
    capabilities::{CapabilityProvider, Dispatcher, NullDispatcher},
    capability_provider,
    core::{OP_BIND_ACTOR, OP_REMOVE_ACTOR},
    deserialize,
};

#[cfg(not(feature = "static_plugin"))]
capability_provider!(LoggingProvider, LoggingProvider::new);

#[allow(unused)]
const CAPABILITY_ID: &str = "wasmcloud:logging";
const SYSTEM_ACTOR: &str = "system";

const ERROR: &str = "error";
const WARN: &str = "warn";
const INFO: &str = "info";
const DEBUG: &str = "debug";
const TRACE: &str = "trace";

/// Standard output logging implementation of the `wasmcloud:logging` specification
#[derive(Clone)]
pub struct LoggingProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
}

impl Default for LoggingProvider {
    fn default() -> Self {
        if env_logger::try_init().is_err() {}

        LoggingProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
        }
    }
}

impl LoggingProvider {
    /// Creates a new logging provider
    pub fn new() -> Self {
        Self::default()
    }

    fn write_log(
        &self,
        actor: &str,
        log_msg: WriteLogArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        match &*log_msg.level {
            ERROR => error!(target: &log_msg.target, "[{}] {}", actor, log_msg.text),
            WARN => warn!(target: &log_msg.target, "[{}] {}", actor, log_msg.text),
            INFO => info!(target: &log_msg.target, "[{}] {}", actor, log_msg.text),
            DEBUG => debug!(target: &log_msg.target, "[{}] {}", actor, log_msg.text),
            TRACE => trace!(target: &log_msg.target, "[{}] {}", actor, log_msg.text),
            _ => error!("Unknown log level: {}", log_msg.level),
        }
        Ok(vec![])
    }
}

impl CapabilityProvider for LoggingProvider {
    // Invoked by the runtime host to give this provider plugin the ability to communicate
    // with actors
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn Dispatcher>,
    ) -> Result<(), Box<dyn Error + Sync + Send>> {
        info!("Dispatcher configured.");
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
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        trace!("Handling operation `{}` from `{}`", op, actor);
        match op {
            OP_BIND_ACTOR if actor == SYSTEM_ACTOR => Ok(vec![]),
            OP_REMOVE_ACTOR if actor == SYSTEM_ACTOR => Ok(vec![]),
            OP_LOG => self.write_log(actor, deserialize(msg)?),
            _ => Err(format!("Unknown operation: {}", op).into()),
        }
    }

    // No cleanup needed on stop
    fn stop(&self) {}
}
