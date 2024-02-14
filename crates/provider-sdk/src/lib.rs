use core::WrpcTarget;
use std::{borrow::Cow, collections::HashMap, time::Duration};

use async_nats::{ConnectOptions, Event};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

pub mod error;
pub mod provider;
pub mod provider_main;
pub mod rpc_client;

pub use provider::ProviderConnection;
pub use provider_main::{load_host_data, run_provider, start_provider};
pub use rpc_client::RpcClient;
pub use wasmcloud_core as core;
pub use wasmcloud_tracing;

use crate::{
    core::{HealthCheckRequest, HealthCheckResponse, LinkDefinition, WasmCloudEntity},
    error::{InvocationError, InvocationResult},
};

pub const URL_SCHEME: &str = "wasmbus";
/// nats address to use if not included in initial HostData
pub(crate) const DEFAULT_NATS_ADDR: &str = "nats://127.0.0.1:4222";
/// The default timeout for a request to the lattice, in milliseconds
pub const DEFAULT_RPC_TIMEOUT_MILLIS: Duration = Duration::from_millis(2000);

// helper methods for serializing and deserializing
pub fn deserialize<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> InvocationResult<T> {
    rmp_serde::from_slice(buf).map_err(InvocationError::from)
}

pub fn serialize<T: Serialize>(data: &T) -> InvocationResult<Vec<u8>> {
    rmp_serde::to_vec_named(data).map_err(InvocationError::from)
}

/// Returns the rpc topic (subject) name for sending to an actor or provider.
/// A provider entity must have the public_key and link_name fields filled in.
/// An actor entity must have a public_key and an empty link_name.
pub fn rpc_topic(entity: &WrpcTarget, lattice: &str) -> String {
    // todo(vados-cosmonic) we have to take the code from the wasmbus PR into here.
    if !entity.link_name.is_empty() {
        // provider target
        format!(
            "wasmbus.rpc.{}.{}.{}",
            lattice, entity.public_key, entity.link_name
        )
    } else {
        // actor target
        format!("wasmbus.rpc.{}.{}", lattice, entity.public_key)
    }
}

/// Generates a fully qualified wasmbus URL for use in wascap claims.
/// The optional method parameter is used for generating URLs for targets being invoked
pub fn url(target: &crate::core::WrpcTarget) -> String {
    // todo(vados-cosmonic): we can remove this entire function, as
    // claim signing will be removed eventually
    //
    // see: https://github.com/wasmCloud/wasmCloud/issues/1219
    format!("wrpc://{}/{}/{}", target.interface.to_string(), target.link_name,  target.id)
}

/// helper method to add logging to a nats connection. Logs disconnection (warn level), reconnection (info level), error (error), slow consumer, and lame duck(warn) events.
pub fn with_connection_event_logging(opts: ConnectOptions) -> ConnectOptions {
    opts.event_callback(|event| async move {
        match event {
            Event::Disconnected => warn!("nats client disconnected"),
            Event::Connected => info!("nats client connected"),
            Event::ClientError(err) => error!("nats client error: '{:?}'", err),
            Event::ServerError(err) => error!("nats server error: '{:?}'", err),
            Event::SlowConsumer(val) => warn!("nats slow consumer detected ({})", val),
            Event::LameDuckMode => warn!("nats lame duck mode"),
        }
    })
}

/// Context - message passing metadata used by wasmhost Actors and Capability Providers
#[derive(Default, Debug, Clone)]
pub struct Context {
    /// Messages received by a Provider will have actor set to the actor's public key
    pub actor: Option<String>,

    /// A map of tracing context information
    pub tracing: HashMap<String, String>,
}

/// The super trait containing all necessary traits for a provider
pub trait Provider: MessageDispatch + ProviderHandler + Send + Sync + 'static {}

/// Handler for receiving messages from an actor and sending them to the right method for a provider. This will likely be automatically generated but
/// can be overridden if you know what you're doing
#[async_trait]
pub trait MessageDispatch {
    async fn dispatch<'a>(
        &'a self,
        ctx: Context,
        method: String,
        body: Cow<'a, [u8]>,
    ) -> InvocationResult<Vec<u8>>;
}

/// CapabilityProvider handling of messages from host
#[async_trait]
pub trait ProviderHandler: Sync {
    /// Provider should perform any operations needed for a new link, including setting up per-actor
    /// resources, and checking authorization. If the link is allowed, return true, otherwise return
    /// false to deny the link or if there are errors. This message is idempotent - provider must be able to handle
    /// duplicates
    async fn put_link(&self, _ld: &LinkDefinition) -> bool {
        true
    }

    /// Notify the provider that the link is dropped
    async fn delete_link(&self, _actor_id: &str) {}

    /// Perform health check. Called at regular intervals by host
    /// Default implementation always returns healthy
    async fn health_request(&self, _arg: &HealthCheckRequest) -> HealthCheckResponse {
        HealthCheckResponse {
            healthy: true,
            message: None,
        }
    }

    /// Handle system shutdown message
    async fn shutdown(&self) {}
}
