use core::fmt::{self, Debug};
use core::future::Future;
use core::ops::{Bound, Deref};
use core::pin::Pin;
use core::time::Duration;

use anyhow::Result;
use anyhow::{ensure, Context as _};
use futures::{Stream, TryStreamExt as _};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::sync::mpsc;
use tracing::{debug, info_span, instrument, warn, Instrument as _, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use wascap::jwt;
use wascap::wasm::extract_claims;
use wasi_preview1_component_adapter_provider::{
    WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME, WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
};
use wasmtime::component::{types, Linker, ResourceTable, ResourceTableError, ResourceType};
use wasmtime::InstanceAllocationStrategy;
use wasmtime_wasi::{IoView, WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::WasiHttpCtx;
use wrpc_runtime_wasmtime::{
    collect_component_resource_exports, collect_component_resource_imports, link_item, rpc,
    RemoteResource, ServeExt as _, SharedResourceTable, WrpcView,
};

use crate::capability::{self, wrpc};
use crate::experimental::Features;
use crate::Runtime;

pub use bus::{Bus, Error};
pub use bus1_0_0::Bus as Bus1_0_0;
pub use config::Config;
pub use identity::Identity;
pub use logging::Logging;
pub use messaging::v0_2::Messaging as Messaging0_2;
pub use messaging::v0_3::{
    Client as MessagingClient0_3, GuestMessage as MessagingGuestMessage0_3,
    HostMessage as MessagingHostMessage0_3, Messaging as Messaging0_3,
};
pub use secrets::Secrets;

pub(crate) mod blobstore;
mod bus;
mod bus1_0_0;
mod config;
mod http;
mod identity;
mod keyvalue;
mod logging;
pub(crate) mod messaging;
mod secrets;

/// Instance target, which is replaced in wRPC
///
/// This enum represents the original instance import invoked by the component
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ReplacedInstanceTarget {
    /// `wasi:blobstore/blobstore` instance replacement
    BlobstoreBlobstore,
    /// `wasi:blobstore/container` instance replacement
    BlobstoreContainer,
    /// `wasi:keyvalue/atomic` instance replacement
    KeyvalueAtomics,
    /// `wasi:keyvalue/store` instance replacement
    KeyvalueStore,
    /// `wasi:keyvalue/batch` instance replacement
    KeyvalueBatch,
    /// `wasi:keyvalue/watch` instance replacment
    KeyvalueWatch,
    /// `wasi:http/incoming-handler` instance replacement
    HttpIncomingHandler,
    /// `wasi:http/outgoing-handler` instance replacement
    HttpOutgoingHandler,
}

fn is_0_2(version: &str, min_patch: u64) -> bool {
    if let Ok(semver::Version {
        major,
        minor,
        patch,
        pre,
        build,
    }) = version.parse()
    {
        major == 0 && minor == 2 && patch >= min_patch && pre.is_empty() && build.is_empty()
    } else {
        false
    }
}

/// This represents a kind of wRPC invocation error
pub enum InvocationErrorKind {
    /// This occurs when the endpoint is not found, for example as would happen when the runtime
    /// would attempt to call `foo:bar/baz@0.2.0`, but the peer served `foo:bar/baz@0.1.0`.
    NotFound,

    /// An error kind, which will result in a trap in the component
    Trap,
}

/// Implementations of this trait are able to introspect an error returned by wRPC invocations
pub trait InvocationErrorIntrospect {
    /// Classify [`InvocationErrorKind`] of an error returned by wRPC
    fn invocation_error_kind(&self, err: &anyhow::Error) -> InvocationErrorKind;
}

/// [`MinimalHandler`] represents the base traits that a handler must implement to be used with
/// `wasmcloud_runtime`. It is a subset of the [`Handler`] trait, which includes many more traits
/// for capability implementations when running in wasmCloud.
pub trait MinimalHandler: Send + Sync + Clone + 'static {}
impl<T: Send + Sync + Clone + 'static> MinimalHandler for T {}

/// A collection of traits that the host must implement
pub trait Handler:
    wrpc_transport::Invoke<Context = Option<ReplacedInstanceTarget>>
    + Bus
    + Config
    + Logging
    + Secrets
    + Messaging0_2
    + Messaging0_3
    + Identity
    + InvocationErrorIntrospect
    + Send
    + Sync
    + Clone
    + 'static
{
}

impl<
        T: wrpc_transport::Invoke<Context = Option<ReplacedInstanceTarget>>
            + Bus
            + Config
            + Logging
            + Secrets
            + Messaging0_2
            + Messaging0_3
            + Identity
            + InvocationErrorIntrospect
            + Send
            + Sync
            + Clone
            + 'static,
    > Handler for T
{
}

/// Component instance configuration
#[derive(Clone, Debug, Default)]
pub struct ComponentConfig {
    /// Whether components are required to be signed to be executed
    pub require_signature: bool,
}

/// Component Environment Limits for more intricate resource control
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Copy)]
pub struct Limits {
    /// Maximum memory allocation in bytes. None defaults to host runtime limits
    pub max_memory_limit: Option<usize>,
    /// Maximum execution time in seconds. None defaults to host runtime limits.
    pub max_execution_time: Option<u64>,
}
impl Limits {
    /// Converts limits to a string-based key-value map for serialization.
    ///
    /// Only includes fields that have values (non-None). Used for converting
    /// to external configuration formats.
    ///
    /// # Returns
    /// `HashMap` containing string representations of set limits
    #[must_use]
    pub fn to_string_map(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();

        if let Some(memory_limit) = self.max_memory_limit {
            map.insert("max_memory_limit".to_string(), memory_limit.to_string());
        }

        if let Some(execution_time) = self.max_execution_time {
            map.insert("max_execution_time".to_string(), execution_time.to_string());
        }

        map
    }
}

/// Creates limits from a string-based key-value map.
///
/// Parses numeric values from string map, ignoring invalid entries.
/// Missing or unparseable values default to None (no limit).
///
/// # Arguments
/// * `map` - Optional `HashMap` containing string representations of limits
///
/// # Returns
/// Optional Limits struct with parsed values, or None if input is None
#[must_use]
pub fn from_string_map<S: std::hash::BuildHasher>(
    map: Option<&HashMap<String, String, S>>,
) -> Option<Limits> {
    map.map(|map| Limits {
        max_memory_limit: map.get("max_memory_limit").and_then(|s| s.parse().ok()),

        max_execution_time: map.get("max_execution_time").and_then(|s| s.parse().ok()),
    })
}
/// Extracts and validates claims contained within a WebAssembly binary, if present
///
/// # Arguments
///
/// * `wasm` - Bytes that constitute a valid WebAssembly binary
///
/// # Errors
///
/// Fails if either parsing fails, or claims are not valid
///
/// # Returns
/// The token embedded in the component, including the [`jwt::Claims`] and the raw JWT
pub fn claims_token(wasm: impl AsRef<[u8]>) -> anyhow::Result<Option<jwt::Token<jwt::Component>>> {
    let Some(claims) = extract_claims(wasm).context("failed to extract module claims")? else {
        return Ok(None);
    };
    let v = jwt::validate_token::<jwt::Component>(&claims.jwt)
        .context("failed to validate module token")?;
    ensure!(!v.expired, "token expired at `{}`", v.expires_human);
    ensure!(
        !v.cannot_use_yet,
        "token cannot be used before `{}`",
        v.not_before_human
    );
    ensure!(v.signature_valid, "signature is not valid");
    Ok(Some(claims))
}

/// Pre-compiled component [Component], which is cheapily-[Cloneable](Clone)
#[derive(Clone)]
pub struct Component<H>
where
    H: MinimalHandler,
{
    engine: wasmtime::Engine,
    claims: Option<jwt::Claims<jwt::Component>>,
    instance_pre: wasmtime::component::InstancePre<Ctx<H>>,
    host_resources: Arc<HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>>,
    max_execution_time: Duration,
    experimental_features: Features,
    max_memory_limit: usize,
}

/// The [`CustomCtxComponent`] is similar to [`Component`], but it supports passing a custom context that
/// implements the [`BaseCtx`] trait. This is useful for when you want to extend the context with additional
/// host implementations or custom functionality.
#[derive(Clone)]
pub struct CustomCtxComponent<C>
where
    C: BaseCtx,
{
    engine: wasmtime::Engine,
    instance_pre: wasmtime::component::InstancePre<C>,
    #[allow(unused)]
    host_resources: Arc<HashMap<Box<str>, HashMap<Box<str>, (ResourceType, ResourceType)>>>,
    max_execution_time: Duration,
}

impl<C> CustomCtxComponent<C>
where
    C: BaseCtx,
{
    /// Compiles a WebAssembly component using [Runtime]. Modules are not supported with this function.
    ///
    /// It is expected that the `linker_fn` is used to link any and all imports to the component. If
    /// not, then the instantiation of the instance will fail.
    #[instrument(level = "trace", skip_all)]
    pub fn new_with_linker_minimal(
        rt: &Runtime,
        wasm: &[u8],
        linker_fn: impl FnOnce(&mut Linker<C>, &wasmtime::component::Component) -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        if wasmparser::Parser::is_core_wasm(wasm) {
            anyhow::bail!("core modules are not supported in the minimal linker");
        }
        let engine = rt.engine.clone();
        let component = wasmtime::component::Component::new(&engine, wasm)
            .context("failed to compile component")?;

        let mut linker = Linker::new(&engine);

        let ty = component.component_type();
        let mut guest_resources = Vec::new();
        let mut host_resources = BTreeMap::new();
        collect_component_resource_exports(&engine, &ty, &mut guest_resources);
        collect_component_resource_imports(&engine, &ty, &mut host_resources);

        let host_resources = host_resources
            .into_iter()
            .map(|(name, instance)| {
                let instance = instance
                    .into_iter()
                    .map(|(name, ty)| (name, (ty, ResourceType::host::<RemoteResource>())))
                    .collect::<HashMap<_, _>>();
                (name, instance)
            })
            .collect::<HashMap<_, _>>();
        let host_resources = Arc::from(host_resources);

        if !guest_resources.is_empty() {
            warn!("exported component resources are not supported in wasmCloud runtime and will be ignored, use a provider instead to enable this functionality");
        }

        linker_fn(&mut linker, &component)?;

        let instance_pre = linker
            .instantiate_pre(&component)
            .context("failed to pre-instantiate component")?;

        Ok(CustomCtxComponent {
            engine,
            instance_pre,
            host_resources,
            max_execution_time: rt.max_execution_time,
        })
    }

    /// Creates a new component store for instantiation
    pub fn new_store(&self, ctx: C) -> wasmtime::Store<C> {
        let mut store = wasmtime::Store::new(&self.engine, ctx);
        store.set_epoch_deadline(self.max_execution_time.as_secs());
        store
    }

    /// Returns the precompiled component instance
    #[must_use]
    pub fn instance_pre(&self) -> &wasmtime::component::InstancePre<C> {
        &self.instance_pre
    }
}

impl<H> Debug for Component<H>
where
    H: MinimalHandler,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Component")
            .field("claims", &self.claims)
            .field("runtime", &"wasmtime")
            .field("max_execution_time", &self.max_execution_time)
            .finish_non_exhaustive()
    }
}

fn new_store<H: Handler>(
    engine: &wasmtime::Engine,
    handler: H,
    max_execution_time: Duration,
) -> wasmtime::Store<Ctx<H>> {
    let table = ResourceTable::new();
    let wasi = WasiCtxBuilder::new()
        .args(&["main.wasm"]) // TODO: Configure argv[0]
        .inherit_stderr()
        .build();

    let mut store = wasmtime::Store::new(
        engine,
        Ctx {
            handler,
            wasi,
            http: WasiHttpCtx::new(),
            table,
            shared_resources: SharedResourceTable::default(),
            timeout: max_execution_time,
            parent_context: None,
        },
    );
    store.set_epoch_deadline(max_execution_time.as_secs());
    store
}

/// Events sent by [`Component::serve_wrpc`]
#[derive(Clone, Debug)]
pub enum WrpcServeEvent<C> {
    /// `wasi:http/incoming-handler.handle` return event
    HttpIncomingHandlerHandleReturned {
        /// Invocation context
        context: C,
        /// Whether the invocation was successfully handled
        success: bool,
    },
    /// `wasmcloud:messaging/handler.handle-message` return event
    MessagingHandlerHandleMessageReturned {
        /// Invocation context
        context: C,
        /// Whether the invocation was successfully handled
        success: bool,
    },
    /// dynamic export return event
    DynamicExportReturned {
        /// Invocation context
        context: C,
        /// Whether the invocation was successfully handled
        success: bool,
    },
}

/// This represents a [Stream] of incoming invocations.
/// Each item represents processing of a single invocation.
pub type InvocationStream = Pin<
    Box<
        dyn Stream<
                Item = anyhow::Result<
                    Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>,
                >,
            > + Send
            + 'static,
    >,
>;

impl<H> Component<H>
where
    H: MinimalHandler,
{
    /// Compiles a WebAssembly component using [Runtime]. Modules are not supported with this function
    ///
    /// If you need to pass a custom context, use [CustomCtxComponent::new_with_linker_minimal].
    ///
    /// It is expected that the `linker_fn` is used to link any and all imports to the component. If
    /// not, then the instantiation of the instance will fail.
    #[instrument(level = "trace", skip_all)]
    pub fn new_with_linker_minimal(
        rt: &Runtime,
        wasm: &[u8],
        linker_fn: impl FnOnce(&mut Linker<Ctx<H>>) -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        if wasmparser::Parser::is_core_wasm(wasm) {
            anyhow::bail!("core modules are not supported in the minimal linker");
        }
        let engine = rt.engine.clone();
        let claims = None;
        let component = wasmtime::component::Component::new(&engine, wasm)
            .context("failed to compile component")?;

        let mut linker = Linker::new(&engine);
        linker_fn(&mut linker)?;

        let ty = component.component_type();
        let mut guest_resources = Vec::new();
        let mut host_resources = BTreeMap::new();
        collect_component_resource_exports(&engine, &ty, &mut guest_resources);
        collect_component_resource_imports(&engine, &ty, &mut host_resources);

        let host_resources = host_resources
            .into_iter()
            .map(|(name, instance)| {
                let instance = instance
                    .into_iter()
                    .map(|(name, ty)| (name, (ty, ResourceType::host::<RemoteResource>())))
                    .collect::<HashMap<_, _>>();
                (name, instance)
            })
            .collect::<HashMap<_, _>>();
        let host_resources = Arc::from(host_resources);

        if !guest_resources.is_empty() {
            warn!("exported component resources are not supported in wasmCloud runtime and will be ignored, use a provider instead to enable this functionality");
        }

        let instance_pre = linker
            .instantiate_pre(&component)
            .context("failed to pre-instantiate component")?;

        Ok(Component {
            engine,
            claims,
            instance_pre,
            host_resources,
            max_execution_time: rt.max_execution_time,
            experimental_features: rt.experimental_features,
            max_memory_limit: rt.max_linear_memory,
        })
    }
}

impl<H> Component<H>
where
    H: Handler,
{
    /// Extracts [Claims](jwt::Claims) from WebAssembly component and compiles it using [Runtime].
    ///
    /// If `wasm` represents a core Wasm module, then it will first be turned into a component.
    #[instrument(level = "trace", skip_all)]
    pub fn new(rt: &Runtime, wasm: &[u8], limits: Option<Limits>) -> anyhow::Result<Self> {
        Self::new_with_linker(rt, wasm, limits, |linker| {
            wasmtime_wasi::add_to_linker_async(linker)
                .context("failed to link core WASI interfaces")?;
            wasmtime_wasi_http::add_only_http_to_linker_async(linker)
                .context("failed to link `wasi:http`")?;

            capability::blobstore::blobstore::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:blobstore/blobstore`")?;
            capability::blobstore::container::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:blobstore/container`")?;
            capability::blobstore::types::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:blobstore/types`")?;
            capability::config::runtime::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:config/runtime`")?;
            capability::config::store::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:config/store`")?;
            capability::keyvalue::atomics::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:keyvalue/atomics`")?;
            capability::keyvalue::store::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:keyvalue/store`")?;
            capability::keyvalue::batch::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:keyvalue/batch`")?;
            capability::logging::logging::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:logging/logging`")?;
            capability::unversioned_logging::logging::add_to_linker(linker, |ctx| ctx)
                .context("failed to link unversioned `wasi:logging/logging`")?;

            capability::bus1_0_0::lattice::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasmcloud:bus/lattice@1.0.0`")?;
            capability::bus2_0_1::lattice::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasmcloud:bus/lattice@2.0.1`")?;
            capability::bus2_0_1::error::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasmcloud:bus/error@2.0.1`")?;
            capability::messaging0_2_0::types::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasmcloud:messaging/types@0.2.0`")?;
            capability::messaging0_2_0::consumer::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasmcloud:messaging/consumer@0.2.0`")?;
            capability::secrets::reveal::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasmcloud:secrets/reveal`")?;
            capability::secrets::store::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasmcloud:secrets/store`")?;
            // Only link wasmcloud:messaging@v3 if the feature is enabled
            if rt.experimental_features.wasmcloud_messaging_v3 {
                capability::messaging0_3_0::types::add_to_linker(linker, |ctx| ctx)
                    .context("failed to link `wasmcloud:messaging/types@0.3.0`")?;
                capability::messaging0_3_0::producer::add_to_linker(linker, |ctx| ctx)
                    .context("failed to link `wasmcloud:messaging/producer@0.3.0`")?;
                capability::messaging0_3_0::request_reply::add_to_linker(linker, |ctx| ctx)
                    .context("failed to link `wasmcloud:messaging/request-reply@0.3.0`")?;
            }
            // Only link wasmcloud:identity if the workload identity feature is enabled
            if rt.experimental_features.workload_identity_interface {
                capability::identity::store::add_to_linker(linker, |ctx| ctx)
                    .context("failed to link `wasmcloud:identity/store`")?;
            }

            // Only link wrpc:rpc if the RPC feature is enabled
            if rt.experimental_features.rpc_interface {
                rpc::add_to_linker(linker).context("failed to link `wrpc:rpc`")?;
            }

            Ok(())
        })
    }

    /// Extracts [Claims](jwt::Claims) from WebAssembly component and compiles it using [Runtime].
    /// The `linker_fn` is used to link additional interfaces to the component.
    ///
    /// If `wasm` represents a core Wasm module, then it will first be turned into a component.
    #[instrument(level = "trace", skip_all)]
    pub fn new_with_linker(
        rt: &Runtime,
        wasm: &[u8],
        limits: Option<Limits>,
        linker_fn: impl FnOnce(&mut Linker<Ctx<H>>) -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        if wasmparser::Parser::is_core_wasm(wasm) {
            let wasm = wit_component::ComponentEncoder::default()
                .module(wasm)
                .context("failed to set core component module")?
                .adapter(
                    WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME,
                    WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER,
                )
                .context("failed to add WASI preview1 adapter")?
                .encode()
                .context("failed to encode a component from module")?;
            return Self::new(rt, &wasm, limits);
        }
        let engine: wasmtime::Engine = if let Some(limits) = limits {
            if limits.max_memory_limit.is_none() {
                rt.engine.clone()
            } else {
                //use engine_config and create separate engine per component and edit the PoolingAllocatorConfig
                let mut component_pooling_config = rt.pooling_config.clone();
                component_pooling_config.max_memory_size(
                    limits
                        .max_memory_limit
                        .expect("max_memory_limit should be Some"),
                );

                let mut component_engine_config = rt.engine_config.clone();
                component_engine_config.allocation_strategy(InstanceAllocationStrategy::Pooling(
                    component_pooling_config,
                ));

                match wasmtime::Engine::new(&component_engine_config)
                    .context("failed to construct engine")
                {
                    Ok(engine) => engine,
                    Err(e) => {
                        tracing::warn!(err = %e, "failed to construct engine with pooling allocator, falling back to dynamic allocator which may result in slower startup and execution of components.");
                        component_engine_config
                            .allocation_strategy(InstanceAllocationStrategy::OnDemand);
                        wasmtime::Engine::new(&component_engine_config)
                            .context("failed to construct engine")?
                    }
                }
            }
        } else {
            rt.engine.clone()
        };
        let claims_token = claims_token(wasm)?;
        let claims = claims_token.map(|c| c.claims);
        let component = wasmtime::component::Component::new(&engine, wasm)
            .context("failed to compile component")?;

        let mut linker = Linker::new(&engine);

        linker_fn(&mut linker)?;

        let ty = component.component_type();
        let mut guest_resources = Vec::new();
        let mut host_resources = BTreeMap::new();
        collect_component_resource_exports(&engine, &ty, &mut guest_resources);
        collect_component_resource_imports(&engine, &ty, &mut host_resources);
        let io_err_tys = host_resources
            .range::<str, _>((
                Bound::Included("wasi:io/error@0.2"),
                Bound::Excluded("wasi:io/error@0.3"),
            ))
            .map(|(name, instance)| {
                instance
                    .get("error")
                    .copied()
                    .with_context(|| format!("{name} instance import missing `error` resource"))
            })
            .collect::<anyhow::Result<Box<[_]>>>()?;
        let io_pollable_tys = host_resources
            .range::<str, _>((
                Bound::Included("wasi:io/poll@0.2"),
                Bound::Excluded("wasi:io/poll@0.3"),
            ))
            .map(|(name, instance)| {
                instance
                    .get("pollable")
                    .copied()
                    .with_context(|| format!("{name} instance import missing `pollable` resource"))
            })
            .collect::<anyhow::Result<Box<[_]>>>()?;
        let io_input_stream_tys = host_resources
            .range::<str, _>((
                Bound::Included("wasi:io/streams@0.2"),
                Bound::Excluded("wasi:io/streams@0.3"),
            ))
            .map(|(name, instance)| {
                instance.get("input-stream").copied().with_context(|| {
                    format!("{name} instance import missing `input-stream` resource")
                })
            })
            .collect::<anyhow::Result<Box<[_]>>>()?;
        let io_output_stream_tys = host_resources
            .range::<str, _>((
                Bound::Included("wasi:io/streams@0.2"),
                Bound::Excluded("wasi:io/streams@0.3"),
            ))
            .map(|(name, instance)| {
                instance.get("output-stream").copied().with_context(|| {
                    format!("{name} instance import missing `output-stream` resource")
                })
            })
            .collect::<anyhow::Result<Box<[_]>>>()?;
        let rpc_err_ty = host_resources
            .get("wrpc:rpc/error@0.1.0")
            .map(|instance| {
                instance
                    .get("error")
                    .copied()
                    .context("`wrpc:rpc/error@0.1.0` instance import missing `error` resource")
            })
            .transpose()?;
        // TODO: This should include `wasi:http` resources
        let host_resources = host_resources
            .into_iter()
            .map(|(name, instance)| {
                let instance = instance
                    .into_iter()
                    .map(|(name, ty)| {
                        let host_ty = match ty {
                            ty if Some(ty) == rpc_err_ty => ResourceType::host::<rpc::Error>(),
                            ty if io_err_tys.contains(&ty) => {
                                ResourceType::host::<wasmtime_wasi::bindings::io::error::Error>()
                            }
                            ty if io_input_stream_tys.contains(&ty) => ResourceType::host::<
                                wasmtime_wasi::bindings::io::streams::InputStream,
                            >(
                            ),
                            ty if io_output_stream_tys.contains(&ty) => ResourceType::host::<
                                wasmtime_wasi::bindings::io::streams::OutputStream,
                            >(
                            ),
                            ty if io_pollable_tys.contains(&ty) => {
                                ResourceType::host::<wasmtime_wasi::bindings::io::poll::Pollable>()
                            }
                            _ => ResourceType::host::<RemoteResource>(),
                        };
                        (name, (ty, host_ty))
                    })
                    .collect::<HashMap<_, _>>();
                (name, instance)
            })
            .collect::<HashMap<_, _>>();
        let host_resources = Arc::from(host_resources);
        if !guest_resources.is_empty() {
            warn!("exported component resources are not supported in wasmCloud runtime and will be ignored, use a provider instead to enable this functionality");
        }
        for (name, ty) in ty.imports(&engine) {
            // Don't link builtin instances or feature-gated instances if the feature is disabled
            match name.split_once('/').map(|(pkg, suffix)| {
                suffix
                    .split_once('@')
                    .map_or((pkg, suffix, None), |(iface, version)| {
                        (pkg, iface, Some(version))
                    })
            }) {
                Some(
                    ("wasi:blobstore", "blobstore" | "container" | "types", Some("0.2.0-draft"))
                    | ("wasi:config", "runtime" | "store", Some("0.2.0-draft"))
                    | ("wasi:keyvalue", "atomics" | "batch" | "store", Some("0.2.0-draft"))
                    | ("wasi:logging", "logging", None | Some("0.1.0-draft"))
                    | ("wasmcloud:bus", "lattice", Some("1.0.0" | "2.0.0"))
                    | ("wasmcloud:messaging", "consumer" | "types", Some("0.2.0"))
                    | ("wasmcloud:secrets", "reveal" | "store", Some("0.1.0-draft")),
                ) => {}
                Some((
                    "wasi:cli",
                    "environment" | "exit" | "stderr" | "stdin" | "stdout" | "terminal-input"
                    | "terminal-output" | "terminal-stderr" | "terminal-stdin" | "terminal-stdout",
                    Some(version),
                )) if is_0_2(version, 0) => {}
                Some(("wasi:clocks", "monotonic-clock" | "wall-clock", Some(version)))
                    if is_0_2(version, 0) => {}
                Some(("wasi:clocks", "timezone", Some(version))) if is_0_2(version, 1) => {}
                Some(("wasi:filesystem", "preopens" | "types", Some(version)))
                    if is_0_2(version, 0) => {}
                Some((
                    "wasi:http",
                    "incoming-handler" | "outgoing-handler" | "types",
                    Some(version),
                )) if is_0_2(version, 0) => {}
                Some(("wasi:io", "error" | "poll" | "streams", Some(version)))
                    if is_0_2(version, 0) => {}
                Some(("wasi:random", "insecure-seed" | "insecure" | "random", Some(version)))
                    if is_0_2(version, 0) => {}
                Some((
                    "wasi:sockets",
                    "instance-network" | "ip-name-lookup" | "network" | "tcp-create-socket" | "tcp"
                    | "udp-create-socket" | "udp",
                    Some(version),
                )) if is_0_2(version, 0) => {}
                _ if rt.skip_feature_gated_instance(name) => {}
                _ => link_item(
                    &engine,
                    &mut linker.root(),
                    [],
                    Arc::clone(&host_resources),
                    ty,
                    "",
                    name,
                )
                .context("failed to link item")?,
            }
        }
        let instance_pre = linker.instantiate_pre(&component)?;
        // use component specific memorylimit or runtime wide limit
        let max_memory_limit = limits
            .and_then(|l| l.max_memory_limit)
            .unwrap_or(rt.max_linear_memory);
        Ok(Self {
            engine,
            claims,
            instance_pre,
            host_resources,
            max_execution_time: rt.max_execution_time,
            experimental_features: rt.experimental_features,
            max_memory_limit,
        })
    }

    /// Sets maximum execution time for functionality exported by this component.
    /// Values below 1 second will be interpreted as 1 second.
    #[instrument(level = "trace", skip_all)]
    pub fn set_max_execution_time(&mut self, max_execution_time: Duration) -> &mut Self {
        self.max_execution_time = max_execution_time.max(Duration::from_secs(1));
        self
    }

    /// Reads the WebAssembly binary asynchronously and calls [Component::new].
    ///
    /// # Errors
    ///
    /// Fails if either reading `wasm` fails or [Self::new] fails
    #[instrument(level = "trace", skip_all)]
    pub async fn read(rt: &Runtime, mut wasm: impl AsyncRead + Unpin) -> anyhow::Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf)
            .await
            .context("failed to read Wasm")?;
        Self::new(rt, &buf, None)
    }

    /// Reads the WebAssembly binary synchronously and calls [Component::new].
    ///
    /// # Errors
    ///
    /// Fails if either reading `wasm` fails or [Self::new] fails
    #[instrument(level = "trace", skip_all)]
    pub fn read_sync(rt: &Runtime, mut wasm: impl std::io::Read) -> anyhow::Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf).context("failed to read Wasm")?;
        Self::new(rt, &buf, None)
    }

    /// [Claims](jwt::Claims) associated with this [Component].
    #[instrument(level = "trace")]
    pub fn claims(&self) -> Option<&jwt::Claims<jwt::Component>> {
        self.claims.as_ref()
    }

    /// Instantiates the component given a handler and event channel
    pub fn instantiate<C>(
        &self,
        handler: H,
        events: mpsc::Sender<WrpcServeEvent<C>>,
    ) -> Instance<H, C> {
        Instance {
            engine: self.engine.clone(),
            pre: self.instance_pre.clone(),
            handler,
            max_execution_time: self.max_execution_time,
            events,
            experimental_features: self.experimental_features,
            max_memory_limit: self.max_memory_limit,
        }
    }

    /// Serve all exports of this [Component] using supplied [`wrpc_transport::Serve`]
    ///
    /// The returned [Vec] contains an [InvocationStream] per each function exported by the component.
    /// A [`WrpcServeEvent`] containing the incoming [`wrpc_transport::Serve::Context`] will be sent
    /// on completion of each invocation.
    /// The supplied [`Handler`] will be used to satisfy imports.
    #[instrument(level = "debug", skip_all)]
    pub async fn serve_wrpc<S>(
        &self,
        srv: &S,
        handler: H,
        events: mpsc::Sender<WrpcServeEvent<S::Context>>,
    ) -> anyhow::Result<Vec<InvocationStream>>
    where
        S: wrpc_transport::Serve,
        S::Context: Deref<Target = tracing::Span>,
    {
        let max_execution_time = self.max_execution_time;
        let mut invocations = vec![];
        let instance = self.instantiate(handler.clone(), events.clone());
        for (name, ty) in self
            .instance_pre
            .component()
            .component_type()
            .exports(&self.engine)
        {
            match (name, ty) {
                (_, types::ComponentItem::ComponentInstance(..))
                    if name.starts_with("wasi:http/incoming-handler@0.2") =>
                {
                    let instance = instance.clone();
                    let [(_, _, handle)] = wrpc_interface_http::bindings::exports::wrpc::http::incoming_handler::serve_interface(
                        srv,
                        wrpc_interface_http::ServeWasmtime(instance),
                    )
                    .await
                    .context("failed to serve `wrpc:http/incoming-handler`")?;
                    invocations.push(handle);
                }
                (
                    "wasmcloud:messaging/handler@0.2.0"
                    | "wasmcloud:messaging/incoming-handler@0.3.0",
                    types::ComponentItem::ComponentInstance(..),
                ) => {
                    let instance = instance.clone();
                    let [(_, _, handle_message)] =
                        wrpc::exports::wasmcloud::messaging0_2_0::handler::serve_interface(
                            srv, instance,
                        )
                        .await
                        .context("failed to serve `wasmcloud:messaging/handler`")?;
                    invocations.push(handle_message);
                }
                (
                    "wasi:keyvalue/watcher@0.2.0-draft",
                    types::ComponentItem::ComponentInstance(..),
                ) => {
                    let instance = instance.clone();
                    let [(_, _, on_set), (_, _, on_delete)] =
                        wrpc::exports::wrpc::keyvalue::watcher::serve_interface(srv, instance)
                            .await
                            .context("failed to serve `wrpc:keyvalue/watcher`")?;
                    invocations.push(on_set);
                    invocations.push(on_delete);
                }
                (name, types::ComponentItem::ComponentFunc(ty)) => {
                    let engine = self.engine.clone();
                    let handler = handler.clone();
                    let pre = self.instance_pre.clone();
                    debug!(?name, "serving root function");
                    let func = srv
                        .serve_function(
                            move || {
                                let span = info_span!("call_instance_function");
                                let mut store =
                                    new_store(&engine, handler.clone(), max_execution_time);
                                store.data_mut().parent_context = Some(span.context());
                                store
                            },
                            pre,
                            Arc::clone(&self.host_resources),
                            ty,
                            "",
                            name,
                        )
                        .await
                        .context("failed to serve root function")?;
                    let events = events.clone();
                    invocations.push(Box::pin(func.map_ok(move |(cx, res)| {
                        let events = events.clone();
                        let span = cx.deref().clone();
                        Box::pin(
                            async move {
                                let res =
                                    res.instrument(info_span!("handle_instance_function")).await;
                                let success = res.is_ok();
                                if let Err(err) =
                                    events.try_send(WrpcServeEvent::DynamicExportReturned {
                                        context: cx,
                                        success,
                                    })
                                {
                                    warn!(
                                        ?err,
                                        success, "failed to send dynamic root export return event"
                                    );
                                }
                                res
                            }
                            .instrument(span),
                        )
                            as Pin<Box<dyn Future<Output = _> + Send + 'static>>
                    })));
                }
                (_, types::ComponentItem::CoreFunc(_)) => {
                    warn!(name, "serving root core function exports not supported yet");
                }
                (_, types::ComponentItem::Module(_)) => {
                    warn!(name, "serving root module exports not supported yet");
                }
                (_, types::ComponentItem::Component(_)) => {
                    warn!(name, "serving root component exports not supported yet");
                }
                (instance_name, types::ComponentItem::ComponentInstance(ty)) => {
                    for (name, ty) in ty.exports(&self.engine) {
                        match ty {
                            types::ComponentItem::ComponentFunc(ty) => {
                                let engine = self.engine.clone();
                                let handler = handler.clone();
                                let pre = self.instance_pre.clone();
                                debug!(?instance_name, ?name, "serving instance function");
                                let func = srv
                                    .serve_function(
                                        move || {
                                            let span = info_span!("call_instance_function");
                                            let mut store = new_store(
                                                &engine,
                                                handler.clone(),
                                                max_execution_time,
                                            );
                                            store.data_mut().parent_context = Some(span.context());
                                            store
                                        },
                                        pre,
                                        Arc::clone(&self.host_resources),
                                        ty,
                                        instance_name,
                                        name,
                                    )
                                    .await
                                    .context("failed to serve instance function")?;
                                let events = events.clone();
                                invocations.push(Box::pin(func.map_ok(move |(cx, res)| {
                                    let events = events.clone();
                                    let span = cx.deref().clone();
                                    Box::pin(
                                        async move {
                                            let res = res.await;
                                            let success = res.is_ok();
                                            if let Err(err) = events.try_send(
                                                WrpcServeEvent::DynamicExportReturned {
                                                    context: cx,
                                                    success,
                                                },
                                            ) {
                                                warn!(
                                                    ?err,
                                                    success,
                                                    "failed to send dynamic instance export return event"
                                                );
                                            }
                                            res
                                        }
                                        .instrument(span),
                                    )
                                        as Pin<Box<dyn Future<Output = _> + Send + 'static>>
                                })));
                            }
                            types::ComponentItem::CoreFunc(_) => {
                                warn!(
                                    instance_name,
                                    name,
                                    "serving instance core function exports not supported yet"
                                );
                            }
                            types::ComponentItem::Module(_) => {
                                warn!(
                                    instance_name,
                                    name, "serving instance module exports not supported yet"
                                );
                            }
                            types::ComponentItem::Component(_) => {
                                warn!(
                                    instance_name,
                                    name, "serving instance component exports not supported yet"
                                );
                            }
                            types::ComponentItem::ComponentInstance(_) => {
                                warn!(
                                    instance_name,
                                    name, "serving nested instance exports not supported yet"
                                );
                            }
                            types::ComponentItem::Type(_) | types::ComponentItem::Resource(_) => {}
                        }
                    }
                }
                (_, types::ComponentItem::Type(_) | types::ComponentItem::Resource(_)) => {}
            }
        }
        Ok(invocations)
    }
}

impl<H> From<Component<H>> for Option<jwt::Claims<jwt::Component>>
where
    H: Handler,
{
    fn from(Component { claims, .. }: Component<H>) -> Self {
        claims
    }
}

/// Instantiated component
pub struct Instance<H, C>
where
    H: Handler,
{
    engine: wasmtime::Engine,
    pre: wasmtime::component::InstancePre<Ctx<H>>,
    handler: H,
    max_execution_time: Duration,
    events: mpsc::Sender<WrpcServeEvent<C>>,
    experimental_features: Features,
    max_memory_limit: usize,
}

impl<H, C> Clone for Instance<H, C>
where
    H: Handler,
{
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            pre: self.pre.clone(),
            handler: self.handler.clone(),
            max_execution_time: self.max_execution_time,
            events: self.events.clone(),
            experimental_features: self.experimental_features,
            max_memory_limit: self.max_memory_limit,
        }
    }
}

type TableResult<T> = Result<T, ResourceTableError>;

/// The minimal implementation of a context that is required to run a component instance.
pub trait BaseCtx: Debug {
    /// The timeout to use for the duration of the component's invocation
    fn timeout(&self) -> Option<Duration> {
        None
    }

    /// The parent context to use for the component's invocation
    fn parent_context(&self) -> Option<&opentelemetry::Context> {
        None
    }
}

/// Wasmtime Context for a component instance, with access to
/// WASI context, HTTP context, and WRPC Invocation context.
/// This is a low-level API and has to be paired with `Component::new_with_linker`.
pub struct Ctx<H>
where
    H: MinimalHandler,
{
    handler: H,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    shared_resources: SharedResourceTable,
    timeout: Duration,
    parent_context: Option<opentelemetry::Context>,
}

impl<H: MinimalHandler> IoView for Ctx<H> {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl<H: MinimalHandler> WasiView for Ctx<H> {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}

impl<H: Handler> WrpcView for Ctx<H> {
    type Invoke = H;

    fn context(&self) -> H::Context {
        None
    }

    fn client(&self) -> &H {
        &self.handler
    }

    fn shared_resources(&mut self) -> &mut SharedResourceTable {
        &mut self.shared_resources
    }

    fn timeout(&self) -> Option<Duration> {
        Some(self.timeout)
    }
}

impl<H: MinimalHandler> Debug for Ctx<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx").field("runtime", &"wasmtime").finish()
    }
}

impl<H: MinimalHandler> Ctx<H> {
    fn attach_parent_context(&self) {
        if let Some(context) = self.parent_context.as_ref() {
            Span::current().set_parent(context.clone());
        }
    }
}
