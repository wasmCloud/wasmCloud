use ::core::future::Future;
use ::core::time::Duration;

use std::collections::HashMap;

use anyhow::Context as _;
use async_nats::{ConnectOptions, Event};
use provider::invocation_context;
use provider::ProviderInitState;
use tower::ServiceExt;
use tracing::{error, info, warn};
use wrpc_transport_legacy::{AcceptedInvocation, IncomingInvocation, OutgoingInvocation};

pub mod error;
pub mod interfaces;
pub mod provider;

#[cfg(feature = "otel")]
pub mod otel;

pub use provider::{get_connection, load_host_data, run_provider, ProviderConnection};
pub use wasmcloud_core as core;
/// Re-export of types from [`wasmcloud_core`]
pub use wasmcloud_core::{
    HealthCheckRequest, HealthCheckResponse, InterfaceLinkDefinition, WitFunction, WitInterface,
    WitNamespace, WitPackage,
};
pub use wasmcloud_tracing;

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
/// nats address to use if not included in initial `HostData`
pub(crate) const DEFAULT_NATS_ADDR: &str = "nats://127.0.0.1:4222";
/// The default timeout for a request to the lattice, in milliseconds
pub const DEFAULT_RPC_TIMEOUT_MILLIS: Duration = Duration::from_millis(2000);

/// helper method to add logging to a nats connection. Logs disconnection (warn level), reconnection (info level), error (error), slow consumer, and lame duck(warn) events.
#[must_use]
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

/// Context - message passing metadata used by wasmCloud Capability Providers
#[derive(Default, Debug, Clone)]
pub struct Context {
    /// Messages received by a Provider will have component set to the component's ID
    pub component: Option<String>,

    /// A map of tracing context information
    pub tracing: HashMap<String, String>,
}

impl Context {
    /// Get link name from the request.
    ///
    /// While link name should in theory *always* be present, it is not natively included in [`Context`] yet,
    /// so we must retrieve it from headers on the request.
    ///
    /// Note that in certain (older) versions of wasmCloud it is possible for the link name to be missing
    /// though incredibly unlikely (basically, due to a bug). In the event that the link name was *not*
    /// properly stored on the context 'default' (the default link name) is returned as the link name.
    #[must_use]
    pub fn link_name(&self) -> &str {
        self.tracing
            .get("link-name")
            .map(String::as_str)
            .unwrap_or("default")
    }
}

/// Configuration of a link that is passed to a provider
#[non_exhaustive]
pub struct LinkConfig<'a> {
    /// Given that the link was established with the source as this provider,
    /// this is the target ID which should be a component
    pub target_id: &'a str,

    /// Given that the link was established with the target as this provider,
    /// this is the source ID which should be a component
    pub source_id: &'a str,

    /// Name of the link that was provided
    pub link_name: &'a str,

    /// Configuration provided to the provider (either as the target or the source)
    pub config: &'a HashMap<String, String>,

    /// WIT metadata for the link
    pub wit_metadata: (&'a WitNamespace, &'a WitPackage, &'a Vec<WitInterface>),
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

/// Capability Provider handling of messages from host
pub trait Provider<E = anyhow::Error>: Sync {
    /// Initialize the provider
    ///
    /// # Arguments
    ///
    /// * `static_config` - Merged named configuration attached to the provider *prior* to startup
    fn init(
        &self,
        init_config: impl ProviderInitConfig,
    ) -> impl Future<Output = Result<(), E>> + Send {
        let _ = init_config;
        async { Ok(()) }
    }

    /// Receive and handle a link that has been established on the lattice where this provider is the source.
    ///
    /// Implement this when your provider needs to call other components.
    ///
    /// [Links](https://wasmcloud.com/docs/concepts/runtime-linking) are uni-directional -- a "source"
    /// operates as one end of the link, linking to a "target". When a link is created on the lattice, and
    /// this provider is the source, this method is called.
    fn receive_link_config_as_source(
        &self,
        config: LinkConfig<'_>,
    ) -> impl Future<Output = Result<(), E>> + Send {
        let _ = config;
        async { Ok(()) }
    }

    /// Receive and handle a link that has been established on the lattice where this provider is the target.
    ///
    /// Implement this when your provider is called by other components.
    ///
    /// [Links](https://wasmcloud.com/docs/concepts/runtime-linking) are uni-directional -- a "source"
    /// operates as one end of the link, linking to a "target". When a link is created on the lattice, and
    /// this provider is the target, this method is called.
    fn receive_link_config_as_target(
        &self,
        config: LinkConfig<'_>,
    ) -> impl Future<Output = Result<(), E>> + Send {
        let _ = config;
        async { Ok(()) }
    }

    /// Notify the provider that the link is dropped
    fn delete_link(&self, component_id: &str) -> impl Future<Output = Result<(), E>> + Send {
        let _ = component_id;
        async { Ok(()) }
    }

    /// Notify the provider that the link is dropped where the provider is the target
    fn delete_link_as_target(
        &self,
        component_id: &str,
    ) -> impl Future<Output = Result<(), E>> + Send {
        let _ = component_id;
        async { Ok(()) }
    }

    /// Notify the provider that the link is dropped where the provider is the source
    fn delete_link_as_source(
        &self,
        component_id: &str,
    ) -> impl Future<Output = Result<(), E>> + Send {
        let _ = component_id;
        async { Ok(()) }
    }

    /// Perform health check. Called at regular intervals by host
    /// Default implementation always returns healthy
    fn health_request(
        &self,
        _arg: &HealthCheckRequest,
    ) -> impl Future<Output = Result<HealthCheckResponse, E>> + Send {
        async {
            Ok(HealthCheckResponse {
                healthy: true,
                message: None,
            })
        }
    }

    /// Handle system shutdown message
    fn shutdown(&self) -> impl Future<Output = Result<(), E>> + Send {
        async { Ok(()) }
    }
}

#[derive(Clone, Debug)]
pub struct WrpcClient(pub wasmcloud_core::wrpc::LegacyClient);

impl wrpc_transport_legacy::Client for WrpcClient {
    type Context = Option<Context>;
    type Subject = <wasmcloud_core::wrpc::LegacyClient as wrpc_transport_legacy::Client>::Subject;
    type Subscriber =
        <wasmcloud_core::wrpc::LegacyClient as wrpc_transport_legacy::Client>::Subscriber;
    type Transmission =
        <wasmcloud_core::wrpc::LegacyClient as wrpc_transport_legacy::Client>::Transmission;
    type Acceptor = <wasmcloud_core::wrpc::LegacyClient as wrpc_transport_legacy::Client>::Acceptor;
    type Invocation =
        <wasmcloud_core::wrpc::LegacyClient as wrpc_transport_legacy::Client>::Invocation;
    type InvocationStream<Ctx, T, Tx: wrpc_transport_legacy::Transmitter> =
        <wasmcloud_core::wrpc::LegacyClient as wrpc_transport_legacy::Client>::InvocationStream<
            Ctx,
            T,
            Tx,
        >;

    fn serve<Ctx, T, Tx, S, Fut>(
        &self,
        instance: &str,
        name: &str,
        svc: S,
    ) -> impl Future<Output = anyhow::Result<Self::InvocationStream<Ctx, T, Tx>>>
    where
        Tx: wrpc_transport_legacy::Transmitter,
        S: tower::Service<
                IncomingInvocation<Self::Context, Self::Subscriber, Self::Acceptor>,
                Future = Fut,
            > + Send
            + Clone
            + 'static,
        Fut: Future<Output = Result<AcceptedInvocation<Ctx, T, Tx>, anyhow::Error>> + Send,
    {
        self.0.serve(
            instance,
            name,
            svc.map_request(
                |IncomingInvocation {
                     context,
                     payload,
                     param_subject,
                     error_subject,
                     handshake_subject,
                     subscriber,
                     acceptor,
                 }: IncomingInvocation<Option<_>, _, _>| {
                    IncomingInvocation {
                        context: context.as_ref().map(invocation_context),
                        payload,
                        param_subject,
                        error_subject,
                        handshake_subject,
                        subscriber,
                        acceptor,
                    }
                },
            ),
        )
    }

    fn new_invocation(
        &self,
    ) -> OutgoingInvocation<Self::Invocation, Self::Subscriber, Self::Subject> {
        self.0.new_invocation()
    }
}
