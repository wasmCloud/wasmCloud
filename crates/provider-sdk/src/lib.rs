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
///
/// A provider entity must have the public_key and link_name fields filled in.
/// An actor entity must have a public_key and an empty link_name.
///
/// For wRPC, this function requires slightly more information, and ends up
/// sending to a completely separate place
pub fn rpc_topic(
    entity: &WasmCloudEntity,
    lattice: &str,
    method: &str,
    wrpc_version: &str,
) -> InvocationResult<String> {
    let pubkey = &entity.public_key;

    // Extract the WIT-specific parts from the method
    let mut split = method.split(':');
    let wit_ns = split.next();
    let wit_pkg_and_iface = split.next().and_then(|rhs| rhs.split_once('/'));
    let (wit_ns, wit_pkg, wit_iface, wit_fn) = match (wit_ns, wit_pkg_and_iface) {
        (Some(wit_ns), Some((wit_pkg, wit_iface_and_fn))) => {
            match wit_iface_and_fn.split_once(".") {
                Some((wit_iface, wit_fn)) => (wit_ns, wit_pkg, wit_iface, wit_fn),
                _ => {
                    return Err(InvocationError::Unexpected(format!(
                        "failed to convert WIT invocation for method [{method}]"
                    )));
                }
            }
        }
        _ => {
            return Err(InvocationError::Unexpected(format!(
                "failed to convert WIT invocation for method [{method}]"
            )));
        }
    };

    Ok(format!(
        "{lattice}.{pubkey}.{wrpc_version}.{wit_ns}:{wit_pkg}/{wit_iface}.{wit_fn}"
    ))
}

/// Generates a fully qualified wasmbus URL for use in wascap claims. The optional method parameter is used for generating URLs for targets being invoked
pub fn url(entity: &crate::core::WasmCloudEntity, method: Option<&str>) -> String {
    // Magic char: First char of an actor public key is M
    let raw_url = if entity.public_key.to_uppercase().starts_with('M') {
        format!("{}://{}", URL_SCHEME, entity.public_key)
    } else {
        format!(
            "{}://{}/{}/{}",
            URL_SCHEME,
            entity
                .contract_id
                .replace(':', "/")
                .replace(' ', "_")
                .to_lowercase(),
            entity.link_name.replace(' ', "_").to_lowercase(),
            entity.public_key
        )
    };
    if let Some(m) = method {
        format!("{}/{}", raw_url, m)
    } else {
        raw_url
    }
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
/// In the case of enabling wrpc, the Wrpc
pub trait Provider: MessageDispatch + ProviderHandler + WitRpc + Send + Sync + 'static {}

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

/// Human readable name of a [`wit_parser::WorldKey`] which includes interface ID if necessary
/// see: https://docs.rs/wit-parser/latest/wit_parser/struct.Resolve.html#method.name_world_key
pub type WorldKeyName = String;

/// WIT function name
pub type WitFunctionName = String;

/// A NATS subject which is used for wRPC
pub type WrpcNatsSubject = String;

/// A trait for providers that are powered by WIT contracts and communicate with wRPC
///
/// Providers are responsible for carrying the contents of their `wit`
/// directories so they can be made available to code (ex. in `provider-sdk`)
#[async_trait]
pub trait WitRpc {
    /// Produces a mapping of NATS subjects to functions that can be invoked by the provider
    async fn incoming_wrpc_invocations_by_subject(
        &self,
        _lattice_name: impl AsRef<str> + Send,
        _component_id: impl AsRef<str> + Send,
        _wrpc_version: impl AsRef<str> + Send,
    ) -> crate::error::ProviderInitResult<
        HashMap<
            WrpcNatsSubject,
            (WorldKeyName, WitFunctionName, ()), // TODO: replace () with wrpc_types::DynamicFunction
        >,
    > {
        Ok(HashMap::new())
    }
}
