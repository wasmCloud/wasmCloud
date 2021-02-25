#![doc(html_logo_url = "https://avatars2.githubusercontent.com/u/52050279?s=200&v=4")]

//! # wasmCloud Provider Core
//!
//! This library provides the core set of types and associated functions used for
//! the common set of functionality required for the wasmCloud host to manipulate
//! capability providers and for developers to create their own providers.
//!
//! # Example
//! The following illustrates an example of the simplest capability provider
//!```
//!
//! use wasmcloud_provider_core as provider;
//! use wasmcloud_actor_core as actor;
//! use provider::{CapabilityProvider, Dispatcher, NullDispatcher, serialize,
//!             core::{OP_BIND_ACTOR, OP_HEALTH_REQUEST, OP_REMOVE_ACTOR, SYSTEM_ACTOR}};
//! use actor::{CapabilityConfiguration, HealthCheckResponse};
//! use std::sync::{Arc, RwLock};
//! use std::error::Error;
//!
//! // Hello world implementation of the `demo:hello` capability provider
//! #[derive(Clone)]
//! pub struct HelloProvider {
//!     dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
//! }
//!
//! const OP_HELLO: &str = "DoHello";
//!
//! impl Default for HelloProvider {
//!     fn default() -> Self {
//!         HelloProvider {
//!             dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
//!         }
//!     }
//! }
//!
//! impl CapabilityProvider for HelloProvider {
//!     // Invoked by the runtime host to give this provider plugin the ability to communicate
//!     // with actors
//!     fn configure_dispatch(
//!         &self,
//!         dispatcher: Box<dyn Dispatcher>,
//!         ) -> Result<(), Box<dyn Error + Sync + Send>> {
//!        
//!         let mut lock = self.dispatcher.write().unwrap();
//!         *lock = dispatcher;
//!         Ok(())
//!     }
//!
//!     // Invoked by host runtime to allow an actor to make use of the capability
//!     // All providers MUST handle the "configure" message, even if no work will be done
//!     fn handle_call(
//!            &self,
//!            actor: &str,
//!            op: &str,
//!            msg: &[u8],
//!        ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
//!
//!        match op {
//!            OP_BIND_ACTOR if actor == SYSTEM_ACTOR => Ok(vec![]),
//!            OP_REMOVE_ACTOR if actor == SYSTEM_ACTOR => Ok(vec![]),
//!            OP_HEALTH_REQUEST if actor == SYSTEM_ACTOR =>
//!                Ok(serialize(HealthCheckResponse {
//!                  healthy: true,
//!                  message: "".to_string(),
//!                })
//!               .unwrap()),
//!            OP_HELLO => Ok(b"Hello, World".to_vec()),
//!            _ => Err(format!("Unknown operation: {}", op).into()),
//!         }
//!     }
//!
//!        // No cleanup needed on stop
//!        fn stop(&self) {}
//!    }
//!
//!```
//!

/// The string used for the originator of messages dispatched by the host runtime
pub const SYSTEM_ACTOR: &str = "system";

pub use capabilities::*;

extern crate rmp_serde as rmps;
use rmps::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

/// The agreed-upon standard for payload serialization (message pack)
pub fn serialize<T>(
    item: T,
) -> ::std::result::Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>
where
    T: Serialize,
{
    let mut buf = Vec::new();
    item.serialize(&mut Serializer::new(&mut buf).with_struct_map())?;
    Ok(buf)
}

/// The agreed-upon standard for payload de-serialization (message pack)
pub fn deserialize<'de, T: Deserialize<'de>>(
    buf: &[u8],
) -> ::std::result::Result<T, Box<dyn std::error::Error + Send + Sync>> {
    let mut de = Deserializer::new(Cursor::new(buf));
    match Deserialize::deserialize(&mut de) {
        Ok(t) => Ok(t),
        Err(e) => Err(format!("Failed to de-serialize: {}", e).into()),
    }
}

pub mod capabilities;
pub mod core;
