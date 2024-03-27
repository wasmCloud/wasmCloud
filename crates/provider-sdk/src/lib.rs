use std::collections::HashMap;
use std::time::Duration;

use anyhow::Context as _;
use async_nats::{ConnectOptions, Event};
use async_trait::async_trait;
use core::{ComponentId, LatticeTarget, LinkName};
use provider::ProviderInitState;
use tracing::{error, info, warn};

pub mod error;
pub mod interfaces;
pub mod provider;

/// Re-export of types from [`wasmcloud_core`]
pub use core::{
    HealthCheckRequest, HealthCheckResponse, InterfaceLinkDefinition, WasmCloudEntity, WitFunction,
    WitInterface, WitNamespace, WitPackage,
};
pub use provider::{
    get_connection, load_host_data, run_provider, run_provider_handler, start_provider,
    ProviderConnection,
};
pub use wasmcloud_core as core;
pub use wasmcloud_tracing;

use crate::error::InvocationResult;
pub use crate::error::ProviderOperationResult;

/// Parse an sufficiently specified WIT operation/method into constituent parts.
///
///
/// # Errors
///
/// Returns `Err` if the operation is not of the form "<package>:<ns>/<interface>.<function>"
///
/// # Example
///
/// ```no_test
/// let (wit_ns, wit_pkg, wit_iface, wit_fn) = parse_wit_meta_from_operation(("wasmcloud:bus/guest-config"));
/// #assert_eq!(wit_ns, "wasmcloud")
/// #assert_eq!(wit_pkg, "bus")
/// #assert_eq!(wit_iface, "iface")
/// #assert_eq!(wit_fn, None)
/// let (wit_ns, wit_pkg, wit_iface, wit_fn) = parse_wit_meta_from_operation(("wasmcloud:bus/guest-config.get"));
/// #assert_eq!(wit_ns, "wasmcloud")
/// #assert_eq!(wit_pkg, "bus")
/// #assert_eq!(wit_iface, "iface")
/// #assert_eq!(wit_fn, Some("get"))
/// ```
pub fn parse_wit_meta_from_operation(
    operation: impl AsRef<str>,
) -> anyhow::Result<(WitNamespace, WitPackage, WitInterface, Option<WitFunction>)> {
    let operation = operation.as_ref();
    let (ns_and_pkg, interface_and_func) = operation
        .rsplit_once('/')
        .context("failed to parse operation")?;
    let (wit_iface, wit_fn) = interface_and_func
        .split_once('.')
        .context("interface and function should be specified")?;
    let (wit_ns, wit_pkg) = ns_and_pkg
        .rsplit_once(':')
        .context("failed to parse operation for WIT ns/pkg")?;
    Ok((
        wit_ns.into(),
        wit_pkg.into(),
        wit_iface.into(),
        if wit_fn.is_empty() {
            None
        } else {
            Some(wit_fn.into())
        },
    ))
}

pub const URL_SCHEME: &str = "wasmbus";
/// nats address to use if not included in initial HostData
pub(crate) const DEFAULT_NATS_ADDR: &str = "nats://127.0.0.1:4222";
/// The default timeout for a request to the lattice, in milliseconds
pub const DEFAULT_RPC_TIMEOUT_MILLIS: Duration = Duration::from_millis(2000);

/// Returns the rpc topic (subject) name for sending to an actor or provider.
/// A provider entity must have the public_key and link_name fields filled in.
/// An actor entity must have a public_key and an empty link_name.
pub fn rpc_topic(entity: &WasmCloudEntity, lattice: &str) -> String {
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

/// Generates a fully qualified wasmbus URL for use in wascap claims. The optional method parameter is used for generating URLs for targets being invoked
// todo(vados-cosmonic): we can remove this entire function once claim signing is removed
// see: https://github.com/wasmCloud/wasmCloud/issues/1219
pub fn url(entity: &WasmCloudEntity, method: Option<&str>) -> String {
    // NOTE: for wRPC, a couple fields in WasmCloudEntity take on separate meanings:
    // - public_key -> target_id
    // - contract_id -> interface
    format!(
        "wrpc://{}/{}/{}{}",
        entity.contract_id,
        entity.link_name,
        entity.public_key,
        method.map(|m| ["/", m].join("")).unwrap_or_default(),
    )
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
pub trait Provider: ProviderHandler + WrpcNats + WrpcDispatch + Send + Sync + 'static {}

/// Configuration of a link that is passed to a provider
pub trait LinkConfig: Send + Sync {
    /// Given that the link was established with the source as this provider,
    /// get the target ID which should be a component
    fn get_target_id(&self) -> &LatticeTarget;

    /// Given that the link was established with the target as this provider,
    /// get the source ID which should be a component
    fn get_source_id(&self) -> &ComponentId;

    /// Get the name of the link that was provided
    fn get_link_name(&self) -> &LinkName;

    /// Get the configuration provided to the provider (either as the target or the source)
    fn get_config(&self) -> &HashMap<String, String>;
}

impl LinkConfig
    for (
        ComponentId,
        LatticeTarget,
        LinkName,
        HashMap<String, String>,
    )
{
    fn get_source_id(&self) -> &ComponentId {
        &self.0
    }

    fn get_target_id(&self) -> &LatticeTarget {
        &self.1
    }

    fn get_link_name(&self) -> &LinkName {
        &self.2
    }

    fn get_config(&self) -> &HashMap<String, String> {
        &self.3
    }
}

impl LinkConfig
    for (
        &ComponentId,
        &LatticeTarget,
        &LinkName,
        &HashMap<String, String>,
    )
{
    fn get_source_id(&self) -> &ComponentId {
        self.0
    }

    fn get_target_id(&self) -> &LatticeTarget {
        self.1
    }

    fn get_link_name(&self) -> &LinkName {
        self.2
    }

    fn get_config(&self) -> &HashMap<String, String> {
        self.3
    }
}

/// Configuration object is made available when a provider is started, to assist in init
///
/// This trait exists to both obscure the underlying implementation and control what information
/// is made available
pub trait ProviderInitConfig: Send + Sync {
    /// Get host-configured provider ID.
    ///
    /// This value may not be knowable to the provider at build time but must be known by runtime.
    fn get_provider_id(&self) -> &str;

    /// Retrieve the configuration for the provider available at initialization time.
    ///
    /// This normally consists of named configuration that were set for the provider,
    /// merged, and received from the host *before* the provider has started initialization.
    fn get_config(&self) -> &HashMap<String, String>;
}

impl ProviderInitConfig for &ProviderInitState {
    fn get_provider_id(&self) -> &str {
        &self.provider_key
    }

    fn get_config(&self) -> &HashMap<String, String> {
        &self.config
    }
}

/// CapabilityProvider handling of messages from host
#[async_trait]
pub trait ProviderHandler: Sync {
    /// Initialize the provider
    ///
    /// # Arguments
    ///
    /// * `static_config` - Merged named configuration attached to the provider *prior* to startup
    async fn init(&self, _init_config: impl ProviderInitConfig) -> ProviderOperationResult<()> {
        Ok(())
    }

    /// Receive and handle a link that has been established on the lattice where this provider is the source.
    ///
    /// Implement this when your provider needs to call other components.
    ///
    /// [Links](https://wasmcloud.com/docs/concepts/runtime-linking) are uni-directional -- a "source"
    /// operates as one end of the link, linking to a "target". When a link is created on the lattice, and
    /// this provider is the source, this method is called.
    async fn receive_link_config_as_source(
        &self,
        _config: impl LinkConfig,
    ) -> ProviderOperationResult<()> {
        Ok(())
    }

    /// Receive and handle a link that has been established on the lattice where this provider is the target.
    ///
    /// Implement this when your provider is called by other components.
    ///
    /// [Links](https://wasmcloud.com/docs/concepts/runtime-linking) are uni-directional -- a "source"
    /// operates as one end of the link, linking to a "target". When a link is created on the lattice, and
    /// this provider is the target, this method is called.
    async fn receive_link_config_as_target(
        &self,
        _config: impl LinkConfig,
    ) -> ProviderOperationResult<()> {
        Ok(())
    }

    /// Notify the provider that the link is dropped
    async fn delete_link(&self, _actor_id: &str) -> ProviderOperationResult<()> {
        Ok(())
    }

    /// Perform health check. Called at regular intervals by host
    /// Default implementation always returns healthy
    async fn health_request(
        &self,
        _arg: &HealthCheckRequest,
    ) -> ProviderOperationResult<HealthCheckResponse> {
        Ok(HealthCheckResponse {
            healthy: true,
            message: None,
        })
    }

    /// Handle system shutdown message
    async fn shutdown(&self) -> ProviderOperationResult<()> {
        Ok(())
    }
}

/// Human readable name of a [`wit_parser::WorldKey`] which includes interface ID if necessary
/// see: https://docs.rs/wit-parser/latest/wit_parser/struct.Resolve.html#method.name_world_key
pub type WorldKeyName = String;

/// A NATS subject which is used for wRPC, normally of the shape `<lattice>.<target id>.wrpc.0.0.1.<interface>.<operation>`
pub type WrpcNatsSubject = String;

pub type WrpcInvocationLookup =
    HashMap<WrpcNatsSubject, (WorldKeyName, WitFunction, wrpc_types::DynamicFunction)>;

/// A trait for providers that are powered by WIT contracts and communicate with wRPC
///
/// Providers are responsible for converting the contents of their WIT files and making
/// a list of invocations available as a lookup that is:
///
/// - Keyed by the wRPC subject to listen on
/// - Contains tuples with:
///   - The appropriate world key name (which includes interface ID -- see [`wit_parser::WorldKey`])
///   - The WIT function name
///   - A callable [`wrpc_types::DynamicFunction`]
///
/// It is up to the host to interpret this information and build necessary lookups/structures to negotiate
/// lattice operations on behalf of the provider.
#[async_trait]
pub trait WrpcNats {
    /// Given a lattice name, produces a mapping of wRPC-compatible subjects (`wrpc_transport::Subject`) to functions that can be invoked by the provider
    ///
    /// # Arguments
    ///
    /// * `lattice_name` - The name of the lattice invocations will be addressed to. This can only be known at runtime after provider instantiation
    /// * `target_id` - The target that represents this provider (ex. a stringified public `nkey`). The target ID may not uniquely identify this provider -- there may be other providers with the same target.
    /// * `wrpc_version` - The version of wRPC that is intended to be used
    async fn incoming_wrpc_invocations_by_subject(
        &self,
        _lattice_name: impl AsRef<str> + Send,
        _target_id: impl AsRef<str> + Send,
        _wrpc_version: impl AsRef<str> + Send,
    ) -> crate::error::ProviderInitResult<WrpcInvocationLookup> {
        Ok(HashMap::new())
    }
}

/// todo: invert this so that the provider takes the wrpc_transport::Client and then hooks up it's own handlers

/// Handler for dispatching invocations that come via wRPC
#[async_trait]
pub trait WrpcDispatch {
    /// Dispatch a single invocation that came in over wRPC
    async fn dispatch_wrpc_dynamic<'a>(
        &'a self,
        ctx: Context,
        operation: String,
        params: Vec<wrpc_transport::Value>,
    ) -> InvocationResult<Vec<u8>>;
}
