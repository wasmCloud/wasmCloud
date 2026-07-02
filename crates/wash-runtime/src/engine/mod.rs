//! WebAssembly component engine for executing workloads.
//!
//! This module provides the core engine functionality for compiling and executing
//! WebAssembly components. The [`Engine`] is responsible for:
//!
//! - Compiling WebAssembly components using wasmtime
//! - Initializing workloads with their components and dependencies
//! - Managing volume mounts and resource configurations
//! - Setting up WASI and HTTP interfaces for components
//!
//! # Key Types
//!
//! - [`Engine`] - The main engine for WebAssembly execution
//! - [`EngineBuilder`] - Builder for configuring engine settings
//! - [`WorkloadComponent`] - Individual components within a workload
//!
//! # Example
//!
//! ```no_run
//! use wash_runtime::engine::Engine;
//! use wash_runtime::types::Workload;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let engine = Engine::builder().build()?;
//! let workload = Workload {
//!     namespace: "default".to_string(),
//!     name: "my-workload".to_string(),
//!     // ... other fields
//! #   annotations: std::collections::HashMap::new(),
//! #   service: None,
//! #   components: vec![],
//! #   host_interfaces: vec![],
//! #   volumes: vec![],
//! };
//!
//! let unresolved = engine.initialize_workload("workload-1", workload)?;
//! // ... bind to plugins and resolve
//! # Ok(())
//! # }
//! ```

use std::hash::Hash;
use std::time::Duration;

use crate::sockets::loopback;
use anyhow::{Context, bail};
use moka::sync::Cache;
use tracing::{instrument, warn};
use wasmtime::PoolingAllocationConfig;
use wasmtime::component::{Component, Linker};

use crate::engine::ctx::SharedCtx;
#[cfg(feature = "wasi-tls")]
use crate::engine::ctx::SharedTlsProvider;
use crate::engine::workload::{UnresolvedWorkload, WorkloadComponent, WorkloadService};
use crate::types::{EmptyDirVolume, HostPathVolume, VolumeType, Workload};
use std::env;
use std::str::FromStr;
use std::{path::PathBuf, sync::Arc};
use wasmtime_wasi::WasiView;

/// Add all WASI interfaces to the linker, using upstream for non-socket interfaces
/// and our custom socket implementation (with loopback support) for socket interfaces.
/// Both P2 and P3 bindings are registered.
fn add_wasi_to_linker(linker: &mut Linker<SharedCtx>) -> anyhow::Result<()> {
    use wasmtime_wasi::p2::bindings::{cli, clocks, filesystem, random, sockets};

    // IO interfaces (error, poll, streams)
    wasmtime_wasi_io::add_to_linker_async(linker)?;

    // Filesystem (async version)
    filesystem::types::add_to_linker::<SharedCtx, wasmtime_wasi::filesystem::WasiFilesystem>(
        linker,
        <SharedCtx as wasmtime_wasi::filesystem::WasiFilesystemView>::filesystem,
    )?;
    filesystem::preopens::add_to_linker::<SharedCtx, wasmtime_wasi::filesystem::WasiFilesystem>(
        linker,
        <SharedCtx as wasmtime_wasi::filesystem::WasiFilesystemView>::filesystem,
    )?;

    // Clocks
    clocks::wall_clock::add_to_linker::<SharedCtx, wasmtime_wasi::clocks::WasiClocks>(
        linker,
        <SharedCtx as wasmtime_wasi::clocks::WasiClocksView>::clocks,
    )?;
    clocks::monotonic_clock::add_to_linker::<SharedCtx, wasmtime_wasi::clocks::WasiClocks>(
        linker,
        <SharedCtx as wasmtime_wasi::clocks::WasiClocksView>::clocks,
    )?;

    // Random
    random::random::add_to_linker::<SharedCtx, wasmtime_wasi::random::WasiRandom>(linker, |t| {
        t.ctx().ctx.random()
    })?;
    random::insecure::add_to_linker::<SharedCtx, wasmtime_wasi::random::WasiRandom>(linker, |t| {
        t.ctx().ctx.random()
    })?;
    random::insecure_seed::add_to_linker::<SharedCtx, wasmtime_wasi::random::WasiRandom>(
        linker,
        |t| t.ctx().ctx.random(),
    )?;

    // CLI
    cli::exit::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::environment::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::stdin::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::stdout::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::stderr::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::terminal_input::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::terminal_output::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::terminal_stdin::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::terminal_stdout::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;
    cli::terminal_stderr::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        <SharedCtx as wasmtime_wasi::cli::WasiCliView>::cli,
    )?;

    // Socket interfaces — use OUR implementation with loopback support
    sockets::tcp::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        ctx::extract_sockets,
    )?;
    sockets::udp::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        ctx::extract_sockets,
    )?;
    sockets::tcp_create_socket::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        ctx::extract_sockets,
    )?;
    sockets::udp_create_socket::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        ctx::extract_sockets,
    )?;
    sockets::instance_network::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        ctx::extract_sockets,
    )?;
    let net_options = sockets::network::LinkOptions::default();
    sockets::network::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        &net_options,
        ctx::extract_sockets,
    )?;
    sockets::ip_name_lookup::add_to_linker::<SharedCtx, crate::sockets::WasiSockets>(
        linker,
        ctx::extract_sockets,
    )?;

    // CLI, clocks, filesystem, random — upstream P3 add_to_linker
    wasmtime_wasi::p3::cli::add_to_linker(linker)?;
    wasmtime_wasi::p3::clocks::add_to_linker(linker)?;
    wasmtime_wasi::p3::filesystem::add_to_linker(linker)?;
    wasmtime_wasi::p3::random::add_to_linker(linker)?;

    // Sockets with our custom P3 implementation (with loopback)
    crate::sockets::add_p3_to_linker(linker)?;

    // wasi:tls@0.3.0-draft (p3).
    #[cfg(feature = "wasi-tls")]
    wasmtime_wasi_tls::p3::add_to_linker(linker)?;

    Ok(())
}

/// Detect whether a component targets WASIP3 by checking for `@0.3` in WASI imports/exports.
/// This is a pure detection function used to pick the P2 vs P3 dispatch path; P3 bindings
/// are always registered on the linker.
pub fn targets_wasip3(component: &Component) -> bool {
    let ty = component.component_type();
    let engine = component.engine();
    ty.imports(engine)
        .any(|(import, _)| import.starts_with("wasi:") && import.contains("@0.3"))
        || ty
            .exports(engine)
            .any(|(export, _)| export.starts_with("wasi:") && export.contains("@0.3"))
}

/// Detect whether a component targets WASIP3 HTTP specifically by checking for
/// `wasi:http` imports/exports with `@0.3`. Used for HTTP dispatch to avoid
/// routing a component that imports `wasi:cli@0.3` but exports `wasi:http@0.2`
/// through the P3 HTTP handler.
pub fn targets_wasip3_http(component: &Component) -> bool {
    let ty = component.component_type();
    let engine = component.engine();
    ty.imports(engine)
        .any(|(name, _)| name.starts_with("wasi:http") && name.contains("@0.3"))
        || ty
            .exports(engine)
            .any(|(name, _)| name.starts_with("wasi:http") && name.contains("@0.3"))
}

pub mod ctx;
mod linked_call;
mod relocate;
mod stream_pump;
mod value;
mod volumes;
pub mod workload;

/// The core WebAssembly engine for executing components and workloads.
///
/// The `Engine` is responsible for compiling WebAssembly components, managing
/// their lifecycle, and providing the runtime environment for execution.
/// It wraps a wasmtime engine with additional functionality for workload management.
#[derive(Clone)]
pub struct Engine {
    // wasmtime engine
    pub(crate) inner: wasmtime::Engine,
    pub(crate) cache: Cache<CacheKey, CacheValue>,
    /// TLS provider override for `wasi:tls` client connections.
    #[cfg(feature = "wasi-tls")]
    pub(crate) tls_provider: Option<SharedTlsProvider>,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("Engine");
        #[cfg(feature = "wasi-tls")]
        s.field("tls_provider", &self.tls_provider.is_some());
        s.finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CacheKey(String);

#[derive(Clone)]
pub struct CacheValue(Component);

impl std::fmt::Debug for CacheValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheValue").finish()
    }
}

impl Engine {
    /// Creates a new [`EngineBuilder`] for configuring an engine.
    ///
    /// # Returns
    /// A default `EngineBuilder` that can be customized with additional configuration.
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// Gets a reference to the inner wasmtime engine.
    pub fn inner(&self) -> &wasmtime::Engine {
        &self.inner
    }

    /// Initializes a workload by validating and preparing all its components.
    ///
    /// This function takes a workload definition and prepares it for execution by:
    /// - Validating service components (if present)
    /// - Setting up volumes (both host path and empty directory types)
    /// - Initializing all components with their resource configurations
    ///
    /// # Arguments
    /// * `id` - Unique identifier for this workload instance
    /// * `workload` - The workload configuration containing components, services, and volumes
    ///
    /// # Returns
    /// An `UnresolvedWorkload` that still needs to be bound to plugins and resolved
    /// before execution.
    ///
    /// # Errors
    /// Returns an error if:
    /// - Service component validation fails
    /// - Volume paths don't exist or aren't accessible
    /// - Component initialization fails
    pub fn initialize_workload(
        &self,
        id: impl AsRef<str>,
        workload: Workload,
    ) -> anyhow::Result<UnresolvedWorkload> {
        let Workload {
            namespace,
            name,
            components,
            service,
            volumes,
            host_interfaces,
            ..
        } = workload;

        // Process and validate volumes - create a lookup map from volume name to validated host path
        let mut validated_volumes = std::collections::HashMap::new();

        for v in volumes {
            let host_path = match v.volume_type {
                VolumeType::HostPath(HostPathVolume { local_path }) => {
                    let path = PathBuf::from(&local_path);
                    if !path.is_dir() {
                        anyhow::bail!(
                            "HostPath volume '{local_path}' does not exist or is not a directory",
                        );
                    }
                    path
                }
                VolumeType::EmptyDir(EmptyDirVolume {}) => {
                    // Create a temporary directory for the empty dir volume
                    let temp_dir = tempfile::tempdir()
                        .context("failed to create temp dir for empty dir volume")?;
                    tracing::debug!(path = ?temp_dir.path(), "created temp dir for empty dir volume");
                    temp_dir.keep()
                }
            };

            // Store the validated volume for later lookup
            validated_volumes.insert(v.name.clone(), host_path);
        }

        let loopback = Arc::default();

        // Iniitalize service
        let service = if let Some(svc) = service {
            match self.initialize_service(
                id.as_ref(),
                &name,
                &namespace,
                svc,
                &validated_volumes,
                Arc::clone(&loopback),
            ) {
                Ok(handle) => {
                    tracing::debug!("successfully initialized service component");
                    Some(handle)
                }
                Err(e) => {
                    tracing::error!(err = ?e, "failed to initialize service component");
                    bail!(e);
                }
            }
        } else {
            None
        };

        // Initialize all components
        let mut workload_components = Vec::new();
        for component in components.into_iter() {
            match self.initialize_workload_component(
                id.as_ref(),
                &name,
                &namespace,
                component,
                &validated_volumes,
                Arc::clone(&loopback),
            ) {
                Ok(handle) => {
                    tracing::debug!("successfully initialized workload component");
                    workload_components.push(handle);
                }
                Err(e) => {
                    tracing::error!(err = ?e, "failed to initialize component");
                    bail!(e);
                }
            }
        }

        let workload = UnresolvedWorkload::new(
            id.as_ref(),
            name,
            namespace,
            service,
            workload_components,
            host_interfaces,
        );

        #[cfg(feature = "wasi-tls")]
        let workload = workload.maybe_with_tls_provider(self.tls_provider.clone());

        Ok(workload)
    }

    #[instrument(name = "initialize_service", skip_all)]
    fn initialize_service(
        &self,
        workload_id: impl AsRef<str>,
        workload_name: impl AsRef<str>,
        workload_namespace: impl AsRef<str>,
        service: crate::types::Service,
        validated_volumes: &std::collections::HashMap<String, PathBuf>,
        loopback: Arc<std::sync::Mutex<loopback::Network>>,
    ) -> anyhow::Result<WorkloadService> {
        // Create a wasmtime component from the bytes
        let wasmtime_component = self
            .load_component_bytes(service.bytes, service.digest)
            .context("failed to create component from bytes")?;

        // Create a linker for this component
        let mut linker: Linker<SharedCtx> = Linker::new(&self.inner);

        // Add WASI interfaces to the linker (with custom socket implementation)
        add_wasi_to_linker(&mut linker).context("failed to add WASI to linker")?;

        // Add HTTP interfaces to the linker if the component uses them
        if uses_wasi_http(&wasmtime_component) {
            wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)
                .map_err(anyhow::Error::from)
                .context("failed to add wasi:http/types to linker")?;
            wasmtime_wasi_http::p3::add_to_linker(&mut linker)
                .map_err(|e| anyhow::anyhow!(e).context("failed to add wasi:http p3 to linker"))?;
        }

        // Build volume mounts for this component by looking up validated volumes
        let mut component_volume_mounts = Vec::new();
        for vm in &service.local_resources.volume_mounts {
            if let Some(host_path) = validated_volumes.get(&vm.name) {
                component_volume_mounts.push((host_path.clone(), vm.clone()));
            } else {
                tracing::warn!(
                    volume = %vm.name,
                    "component references volume that was not found in workload volumes",
                );
            }
        }

        let service = WorkloadService::new(
            workload_id.as_ref(),
            workload_name.as_ref(),
            workload_namespace.as_ref(),
            wasmtime_component,
            linker,
            component_volume_mounts,
            service.local_resources,
            service.max_restarts,
            loopback,
        );

        let world = service.world();

        if !world.exports.iter().any(|iface| {
            iface.namespace == "wasi" && iface.package == "cli" && iface.interfaces.contains("run")
        }) && world.exports.len() != 1
        {
            bail!("Service must export a single interface with the 'run' function");
        }

        // Create the WorkloadService with volume mounts
        Ok(service)
    }

    /// Load a WebAssembly component from raw bytes or yields a previously compiled one.
    #[instrument(name = "load_component_bytes", skip_all, fields(digest = %digest.as_ref().map(|d| d.as_ref()).unwrap_or("none")))]
    fn load_component_bytes(
        &self,
        bytes: impl AsRef<[u8]>,
        digest: Option<impl AsRef<str>>,
    ) -> anyhow::Result<Component> {
        match digest {
            None => {
                tracing::debug!("no digest provided, compiling component without caching");
                let compiled = Component::new(&self.inner, bytes.as_ref())
                    .map_err(anyhow::Error::from)
                    .context("failed to compile component from bytes")?;
                Ok(compiled)
            }
            Some(digest) => {
                let key = CacheKey(digest.as_ref().to_string());
                let inner = &self.inner;
                let bytes_ref = bytes.as_ref();

                self.cache
                    .try_get_with(key, || {
                        Component::new(inner, bytes_ref)
                            .map_err(anyhow::Error::from)
                            .context("failed to compile component from bytes")
                            .map(CacheValue)
                    })
                    .map_err(|e: Arc<anyhow::Error>| {
                        anyhow::anyhow!(e).context("compilation cache error")
                    })
                    .map(|v| v.0)
            }
        }
    }

    /// Initialize a component that is a part of a workload, add wasi@0.2 interfaces (and
    /// wasi:http if the `http` feature is enabled) to the linker.
    #[instrument(name = "initialize_workload_component", skip_all, fields(component.name = %component.name))]
    fn initialize_workload_component(
        &self,
        workload_id: impl AsRef<str>,
        workload_name: impl AsRef<str>,
        workload_namespace: impl AsRef<str>,
        component: crate::types::Component,
        validated_volumes: &std::collections::HashMap<String, PathBuf>,
        loopback: Arc<std::sync::Mutex<loopback::Network>>,
    ) -> anyhow::Result<WorkloadComponent> {
        // Create a wasmtime component from the bytes
        let wasmtime_component = self
            .load_component_bytes(component.bytes, component.digest)
            .context("failed to create component from bytes")?;

        // Create a linker for this component
        let mut linker: Linker<SharedCtx> = Linker::new(&self.inner);

        // Add WASI interfaces to the linker (with custom socket implementation)
        add_wasi_to_linker(&mut linker).context("failed to add WASI to linker")?;

        // Add HTTP interfaces to the linker
        if uses_wasi_http(&wasmtime_component) {
            wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)
                .map_err(anyhow::Error::from)
                .context("failed to add wasi:http/types to linker")?;
            wasmtime_wasi_http::p3::add_to_linker(&mut linker)
                .map_err(|e| anyhow::anyhow!(e).context("failed to add wasi:http p3 to linker"))?;
        }

        // Build volume mounts for this component by looking up validated volumes
        let mut component_volume_mounts = Vec::new();
        for vm in &component.local_resources.volume_mounts {
            if let Some(host_path) = validated_volumes.get(&vm.name) {
                component_volume_mounts.push((host_path.clone(), vm.clone()));
            } else {
                tracing::warn!(
                    volume = %vm.name,
                    "component references volume that was not found in workload volumes",
                );
            }
        }

        // Create the WorkloadComponent with volume mounts
        Ok(WorkloadComponent::new(
            workload_id.as_ref(),
            workload_name.as_ref(),
            workload_namespace.as_ref(),
            component.name,
            wasmtime_component,
            linker,
            component_volume_mounts,
            component.local_resources,
            loopback,
            // TODO: implement pooling and instance limits
            // component.pool_size,
            // component.max_invocations,
        ))
    }
}

/// A wasmtime WebAssembly proposal that can be opted into on the engine.
///
/// Each variant maps to one or more `wasmtime::Config` feature flags. Use it
/// with [`EngineBuilder::with_wasm_proposal`] to enable a bundle of related
/// settings without having to replace the entire config via
/// [`EngineBuilder::with_config`].
///
/// ```no_run
/// # use wash_runtime::engine::{Engine, WasmProposal};
/// # fn example() -> anyhow::Result<()> {
/// let engine = Engine::builder()
///     .with_wasm_proposal(WasmProposal::Gc)
///     .with_wasm_proposal(WasmProposal::Threads)
///     .build()?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum WasmProposal {
    /// Component model async ABI (`stream`/`future`/`error-context` types).
    /// Enables `wasm_component_model_async`. Required for WASIP3.
    ComponentModelAsync,
    /// Component model `(implements ..)` named imports, letting a component
    /// import the same interface multiple times under distinct names so host
    /// plugins can route each independently. Enables
    /// `wasm_component_model_implements`. Requires the backported wasmtime
    /// support, so it is only available with the `wasm_component_model_implements`
    /// crate feature.
    #[cfg(feature = "wasm_component_model_implements")]
    WasmComponentModelImplements,
    /// Garbage collection. Enables `wasm_function_references` (a prerequisite)
    /// and `wasm_gc`.
    Gc,
    /// Exception handling. Enables `wasm_exceptions`.
    ExceptionHandling,
    /// 128-bit wide arithmetic. Enables `wasm_wide_arithmetic`.
    WideArithmetic,
    /// Shared-memory threads. Enables `wasm_threads`.
    Threads,
    /// Tail calls. Enables `wasm_tail_call`.
    TailCall,
}

impl WasmProposal {
    /// Apply this proposal's wasmtime feature flags onto `cfg`.
    fn apply(self, cfg: &mut wasmtime::Config) {
        match self {
            WasmProposal::ComponentModelAsync => {
                cfg.wasm_component_model_async(true);
            }
            #[cfg(feature = "wasm_component_model_implements")]
            WasmProposal::WasmComponentModelImplements => {
                cfg.wasm_component_model_implements(true);
            }
            WasmProposal::Gc => {
                // GC builds on the function-references proposal.
                cfg.wasm_function_references(true);
                cfg.wasm_gc(true);
            }
            WasmProposal::ExceptionHandling => {
                cfg.wasm_exceptions(true);
            }
            WasmProposal::WideArithmetic => {
                cfg.wasm_wide_arithmetic(true);
            }
            WasmProposal::Threads => {
                cfg.wasm_threads(true);
            }
            WasmProposal::TailCall => {
                cfg.wasm_tail_call(true);
            }
        }
    }
}

/// Error returned when a string cannot be parsed into a [`WasmProposal`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseWasmProposalError(String);

impl std::fmt::Display for ParseWasmProposalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "unknown wasm proposal {:?}; expected one of: component-model-async, gc, \
             exception-handling, wide-arithmetic, threads, tail-call",
            self.0
        )
    }
}

impl std::error::Error for ParseWasmProposalError {}

impl FromStr for WasmProposal {
    type Err = ParseWasmProposalError;

    /// Parse a proposal from its kebab-case name. Matching is case-insensitive
    /// and treats `_` and `-` interchangeably, so `gc`, `GC`,
    /// `exception_handling`, and `exception-handling` all parse.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "component-model-async" => Ok(Self::ComponentModelAsync),
            "gc" => Ok(Self::Gc),
            "exception-handling" => Ok(Self::ExceptionHandling),
            "wide-arithmetic" => Ok(Self::WideArithmetic),
            "threads" => Ok(Self::Threads),
            "tail-call" => Ok(Self::TailCall),
            _ => Err(ParseWasmProposalError(s.to_string())),
        }
    }
}

/// Builder for constructing an [`Engine`] with custom configuration.
///
/// The builder pattern allows for flexible configuration of the engine
/// before creation. By default, it enables async support which is required
/// for component execution.
///
/// Settings configured through the builder ([`with_pooling_allocator`],
/// [`with_fuel_consumption`], [`with_wasm_proposal`], …) are layered on top of
/// any base config supplied via [`with_config`], so a custom config can be
/// combined with pooling and proposal flags rather than replacing them.
///
/// [`with_pooling_allocator`]: EngineBuilder::with_pooling_allocator
/// [`with_fuel_consumption`]: EngineBuilder::with_fuel_consumption
/// [`with_wasm_proposal`]: EngineBuilder::with_wasm_proposal
/// [`with_config`]: EngineBuilder::with_config
#[derive(Default)]
pub struct EngineBuilder {
    config: Option<wasmtime::Config>,
    use_pooling_allocator: Option<bool>,
    max_instances: Option<u32>,
    pooling_config: Option<PoolingAllocationConfig>,
    proposals: std::collections::BTreeSet<WasmProposal>,
    compilation_cache_size: Option<u64>,
    compilation_cache_ttl: Option<Duration>,
    fuel_consumption: Option<bool>,
    /// Optional TLS provider override for wasi:tls client connections.
    #[cfg(feature = "wasi-tls")]
    tls_provider: Option<SharedTlsProvider>,
}

impl EngineBuilder {
    /// Creates a new `EngineBuilder` with default configuration.
    ///
    /// # Returns
    /// A new builder instance with default wasmtime configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables or disables the pooling allocator for instance allocation.
    pub fn with_pooling_allocator(mut self, enable: bool) -> Self {
        self.use_pooling_allocator = Some(enable);
        self
    }

    /// Sets the maximum number of instances for the pooling allocator.
    /// This is a 'hint' and can be overridden by environment variables.
    ///
    /// Ignored when a full [`PoolingAllocationConfig`] is supplied via
    /// [`with_pooling_config`](Self::with_pooling_config).
    pub fn with_max_instances(mut self, max: u32) -> Self {
        self.max_instances = Some(max);
        self
    }

    /// Sets a fully-customized [`PoolingAllocationConfig`] for the pooling
    /// allocator, giving callers control over every pooling knob rather than
    /// only the `WASMTIME_POOLING_*` environment variables.
    ///
    /// Supplying a pooling config implies the pooling allocator is enabled and
    /// takes precedence over [`with_max_instances`](Self::with_max_instances).
    /// It is still only applied when the host supports the pooling allocator.
    pub fn with_pooling_config(mut self, config: PoolingAllocationConfig) -> Self {
        self.pooling_config = Some(config);
        self.use_pooling_allocator = Some(true);
        self
    }

    /// Enables an additional WebAssembly proposal on the engine.
    ///
    /// Each [`WasmProposal`] maps to one or more `wasmtime::Config` feature
    /// flags (see the variant docs for the exact flags). Proposals are layered
    /// on top of any base config from [`with_config`](Self::with_config), so a
    /// single extra proposal can be enabled without replacing the whole config.
    /// Calling this repeatedly accumulates proposals; duplicates are ignored.
    pub fn with_wasm_proposal(mut self, proposal: WasmProposal) -> Self {
        self.proposals.insert(proposal);
        self
    }

    /// Enables or disables fuel consumption for the engine.
    ///
    /// When unset, fuel consumption is left at whatever the base config (from
    /// [`with_config`](Self::with_config), or the wasmtime default of disabled)
    /// specifies, rather than being forced off.
    pub fn with_fuel_consumption(mut self, enable: bool) -> Self {
        self.fuel_consumption = Some(enable);
        self
    }

    /// Sets a custom wasmtime configuration to use as the *base* for the engine.
    ///
    /// This config is used as the starting point, and any other builder
    /// settings — pooling allocator, fuel consumption, and
    /// [`WasmProposal`]s — are layered on top of it. This means a custom config
    /// can be combined with [`with_pooling_allocator`](Self::with_pooling_allocator),
    /// [`with_max_instances`](Self::with_max_instances), and
    /// [`with_wasm_proposal`](Self::with_wasm_proposal) rather than being
    /// mutually exclusive with them.
    ///
    /// Unlike the default-config path, the pooling allocator is *not* enabled
    /// by default on top of a custom config. A base config's allocation
    /// strategy is preserved unless pooling is explicitly requested via
    /// [`with_pooling_allocator`](Self::with_pooling_allocator),
    /// [`with_pooling_config`](Self::with_pooling_config), or the
    /// `WASMTIME_POOLING` environment variable.
    ///
    /// # Arguments
    /// * `config` - A wasmtime `Config` object with custom settings
    ///
    /// # Returns
    /// The builder instance for method chaining.
    pub fn with_config(mut self, config: wasmtime::Config) -> Self {
        self.config = Some(config);
        self
    }

    /// Configures a compilation cache for the engine.
    pub fn with_compilation_cache(mut self, size: u64, ttl: Duration) -> Self {
        self.compilation_cache_size = Some(size);
        self.compilation_cache_ttl = Some(ttl);
        self
    }

    /// Override the TLS provider used for `wasi:tls` client connections.
    ///
    /// Use this to plug in an alternative TLS backend, install a custom root
    /// certificate store (corporate CAs, certificate pinning), or integrate
    /// with HSM-backed key material.
    #[cfg(feature = "wasi-tls")]
    pub fn with_tls_provider(mut self, provider: SharedTlsProvider) -> Self {
        self.tls_provider = Some(provider);
        self
    }
}

impl EngineBuilder {
    /// Builds and returns a configured [`Engine`].
    ///
    /// This method finalizes the configuration and creates the engine.
    /// It automatically enables async support which is required for
    /// component execution.
    ///
    /// # Returns
    /// A new `Engine` instance configured with the builder's settings.
    ///
    /// # Errors
    /// Returns an error if the wasmtime engine creation fails.
    pub fn build(mut self) -> anyhow::Result<Engine> {
        #[cfg(feature = "wasi-tls")]
        {
            crate::init_crypto();
            if self.tls_provider.is_none() {
                tracing::warn!(
                    "wasi-tls is enabled but no TLS provider was set; \
                     falling back to wasmtime-wasi-tls default — \
                     set one via EngineBuilder::with_tls_provider"
                );
            }
        }

        // Start from the caller-supplied base config (or wasmtime's default) and
        // layer the builder-managed settings on top of it.
        let has_custom_config = self.config.is_some();
        let mut config = self.config.take().unwrap_or_default();

        // Default the pooling allocator on only when starting from wasmtime's
        // default config. With a caller-supplied base config, leave its
        // allocation strategy untouched unless pooling was explicitly requested
        // (via with_pooling_allocator/with_pooling_config or WASMTIME_POOLING),
        // so a custom strategy is not silently overridden.
        let use_pooling_allocator = self
            .use_pooling_allocator
            .or_else(|| getenv::<bool>("WASMTIME_POOLING"))
            .unwrap_or(!has_custom_config);

        // The pooling allocator can be more efficient for workloads with many short-lived instances
        if use_pooling_allocator && let Ok(true) = is_pooling_allocator_supported() {
            tracing::debug!("using pooling allocator by default");
            let pooling = self
                .pooling_config
                .take()
                .unwrap_or_else(|| new_pooling_config(self.max_instances.unwrap_or(1000)));
            config.allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(pooling));
        } else if use_pooling_allocator {
            tracing::warn!("pooling allocator requested but not supported");
        }

        // Only override fuel consumption when the caller explicitly set it, so a
        // custom base config's setting is otherwise preserved.
        if let Some(fuel) = self.fuel_consumption {
            config.consume_fuel(fuel);
        }

        // WASIP3's async ABI requires the component-model async proposal.
        self.proposals.insert(WasmProposal::ComponentModelAsync);

        // Accept components that import an interface multiple times via the
        // component-model `(implements ..)` annotation, so host plugins can
        // route each named import (e.g. two `wasi:keyvalue/store` imports
        // backed by redis vs NATS) independently. Only available with the
        // backported wasmtime support behind the
        // `wasm_component_model_implements` feature.
        #[cfg(feature = "wasm_component_model_implements")]
        self.proposals
            .insert(WasmProposal::WasmComponentModelImplements);

        for proposal in &self.proposals {
            proposal.apply(&mut config);
        }

        let inner = wasmtime::Engine::new(&config)?;
        let cache = Cache::builder()
            .max_capacity(self.compilation_cache_size.unwrap_or(100))
            .time_to_idle(
                self.compilation_cache_ttl
                    .unwrap_or(Duration::from_secs(600)),
            )
            .build();
        Ok(Engine {
            inner,
            cache,
            #[cfg(feature = "wasi-tls")]
            tls_provider: self.tls_provider,
        })
    }
}

/// Helper function to determine if a component uses wasi:http interfaces
pub fn uses_wasi_http(component: &Component) -> bool {
    imports_wasi_http(component) || exports_wasi_http(component)
}

pub fn exports_wasi_http(component: &Component) -> bool {
    let ty: wasmtime::component::types::Component = component.component_type();
    let engine = component.engine();

    ty.exports(engine)
        .any(|(export, _item)| export.starts_with("wasi:http"))
}

/// Whether a component exports the `wasmcloud:messaging/handler` interface (the
/// inbound message handler the host invokes).
pub fn exports_messaging_handler(component: &Component) -> bool {
    let ty: wasmtime::component::types::Component = component.component_type();
    let engine = component.engine();

    ty.exports(engine)
        .any(|(export, _item)| export.starts_with("wasmcloud:messaging/handler"))
}

pub fn imports_wasi_http(component: &Component) -> bool {
    let ty: wasmtime::component::types::Component = component.component_type();
    let engine = component.engine();

    ty.imports(engine)
        .any(|(import, _item)| import.starts_with("wasi:http"))
}

// TL;DR this is likely best for machines that can handle the large virtual memory requirement of the pooling allocator
// https://github.com/bytecodealliance/wasmtime/blob/b943666650696f1eb7ff8b217762b58d5ef5779d/src/commands/serve.rs#L641-L656
fn is_pooling_allocator_supported() -> anyhow::Result<bool> {
    const BITS_TO_TEST: u32 = 42;
    let mut config = wasmtime::Config::new();
    config.wasm_memory64(true);
    config.memory_reservation(1 << BITS_TO_TEST);
    let engine = wasmtime::Engine::new(&config)?;
    let mut store = wasmtime::Store::new(&engine, ());
    // NB: the maximum size is in wasm pages to take out the 16-bits of wasm
    // page size here from the maximum size.
    let ty = wasmtime::MemoryType::new64(0, Some(1 << (BITS_TO_TEST - 16)));
    Ok(wasmtime::Memory::new(&mut store, ty).is_ok())
}

fn getenv<T>(key: &str) -> Option<T>
where
    T: FromStr,
    T::Err: core::fmt::Debug,
{
    match env::var(key).as_deref().map(FromStr::from_str) {
        Ok(Ok(v)) => Some(v),
        Ok(Err(err)) => {
            warn!(?err, "failed to parse `{key}` value, ignoring");
            None
        }
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(..)) => {
            warn!("`{key}` value is not valid UTF-8, ignoring");
            None
        }
    }
}

fn new_pooling_config(instances: u32) -> PoolingAllocationConfig {
    let mut config = PoolingAllocationConfig::default();
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_UNUSED_WASM_SLOTS") {
        config.max_unused_warm_slots(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_DECOMMIT_BATCH_SIZE") {
        config.decommit_batch_size(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_ASYNC_STACK_KEEP_RESIDENT") {
        config.async_stack_keep_resident(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT") {
        config.linear_memory_keep_resident(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_TABLE_KEEP_RESIDENT") {
        config.table_keep_resident(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_TOTAL_COMPONENT_INSTANCES") {
        config.total_component_instances(v);
    } else {
        config.total_component_instances(instances);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_COMPONENT_INSTANCE_SIZE") {
        config.max_component_instance_size(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_CORE_INSTANCES_PER_COMPONENT") {
        config.max_core_instances_per_component(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_MEMORIES_PER_COMPONENT") {
        config.max_memories_per_component(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_TABLES_PER_COMPONENT") {
        config.max_tables_per_component(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_TOTAL_MEMORIES") {
        config.total_memories(v);
    } else {
        config.total_memories(instances);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_TOTAL_TABLES") {
        config.total_tables(v);
    } else {
        config.total_tables(instances);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_TOTAL_STACKS") {
        config.total_stacks(v);
    } else {
        config.total_stacks(instances);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_TOTAL_CORE_INSTANCES") {
        config.total_core_instances(v);
    } else {
        config.total_core_instances(instances);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_CORE_INSTANCE_SIZE") {
        config.max_core_instance_size(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_TABLES_PER_MODULE") {
        config.max_tables_per_module(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_TABLE_ELEMENTS") {
        config.table_elements(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_MEMORIES_PER_MODULE") {
        config.max_memories_per_module(v);
    }
    if let Some(v) = getenv("WASMTIME_POOLING_MAX_MEMORY_SIZE") {
        config.max_memory_size(v);
    }
    #[cfg(not(windows))]
    if let Some(v) = getenv("WASMTIME_POOLING_TOTAL_GC_HEAPS") {
        config.total_gc_heaps(v);
    } else {
        config.total_gc_heaps(instances);
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    // A custom base config can now be combined with the pooling allocator and
    // instance limits, which previously errored out of `build()`.
    #[test]
    fn custom_config_layers_with_pooling() {
        let mut cfg = wasmtime::Config::default();
        cfg.cranelift_opt_level(wasmtime::OptLevel::Speed);

        Engine::builder()
            .with_config(cfg)
            .with_pooling_allocator(true)
            .with_max_instances(64)
            .build()
            .expect("custom config + pooling should build");
    }

    // A single extra proposal can be enabled on top of a custom config without
    // replacing the whole config.
    #[test]
    fn wasm_proposal_layers_with_custom_config() {
        Engine::builder()
            .with_config(wasmtime::Config::default())
            .with_wasm_proposal(WasmProposal::Gc)
            .build()
            .expect("custom config + GC proposal should build");
    }

    // An externally-supplied pooling config is accepted and applied.
    #[test]
    fn external_pooling_config_builds() {
        let mut pool = PoolingAllocationConfig::default();
        pool.total_component_instances(32);

        Engine::builder()
            .with_pooling_config(pool)
            .build()
            .expect("external pooling config should build");
    }

    // Proposal names parse case-insensitively and accept `_`/`-` interchangeably;
    // unknown names are rejected.
    #[test]
    fn wasm_proposal_from_str() {
        assert_eq!("gc".parse(), Ok(WasmProposal::Gc));
        assert_eq!("GC".parse(), Ok(WasmProposal::Gc));
        assert_eq!(" threads ".parse(), Ok(WasmProposal::Threads));
        assert_eq!(
            "exception_handling".parse(),
            Ok(WasmProposal::ExceptionHandling)
        );
        assert_eq!(
            "component-model-async".parse(),
            Ok(WasmProposal::ComponentModelAsync)
        );
        assert!("nonsense".parse::<WasmProposal>().is_err());
    }
}
