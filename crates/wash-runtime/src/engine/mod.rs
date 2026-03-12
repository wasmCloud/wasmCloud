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
use crate::engine::workload::{UnresolvedWorkload, WorkloadComponent, WorkloadService};
use crate::types::{EmptyDirVolume, HostPathVolume, VolumeType, Workload};
use std::env;
use std::str::FromStr;
use std::{path::PathBuf, sync::Arc};
use wasmtime_wasi::WasiView;

/// Add all WASI@0.2 interfaces to the linker, using upstream for non-socket interfaces
/// and our custom socket implementation (with loopback support) for socket interfaces.
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
    let cli_options = cli::exit::LinkOptions::default();
    cli::exit::add_to_linker::<SharedCtx, wasmtime_wasi::cli::WasiCli>(
        linker,
        &cli_options,
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

    Ok(())
}

pub mod ctx;
mod value;
pub mod workload;

/// The core WebAssembly engine for executing components and workloads.
///
/// The `Engine` is responsible for compiling WebAssembly components, managing
/// their lifecycle, and providing the runtime environment for execution.
/// It wraps a wasmtime engine with additional functionality for workload management.
#[derive(Debug, Clone)]
pub struct Engine {
    // wasmtime engine
    pub(crate) inner: wasmtime::Engine,
    pub(crate) cache: Cache<CacheKey, CacheValue>,
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
    ///
    /// This provides access to the underlying wasmtime engine for advanced use cases.
    ///
    /// # Returns
    /// A reference to the internal `wasmtime::Engine`.
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

        Ok(UnresolvedWorkload::new(
            id.as_ref(),
            name,
            namespace,
            service,
            workload_components,
            host_interfaces,
        ))
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

        // Add WASI@0.2 interfaces to the linker (with custom socket implementation)
        add_wasi_to_linker(&mut linker).context("failed to add WASI to linker")?;

        // Add HTTP interfaces to the linker if feature is enabled and component uses them
        if uses_wasi_http(&wasmtime_component) {
            wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)
                .context("failed to add wasi:http/types to linker")?;
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
                            .context("failed to compile component from bytes")
                            .map(CacheValue)
                    })
                    .map_err(|e| anyhow::anyhow!(e).context("compilation cache error"))
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

        // Add WASI@0.2 interfaces to the linker (with custom socket implementation)
        add_wasi_to_linker(&mut linker).context("failed to add WASI to linker")?;

        // Add HTTP interfaces to the linker
        if uses_wasi_http(&wasmtime_component) {
            wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)
                .context("failed to add wasi:http/types to linker")?;
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

/// Builder for constructing an [`Engine`] with custom configuration.
///
/// The builder pattern allows for flexible configuration of the engine
/// before creation. By default, it enables async support which is required
/// for component execution.
#[derive(Default)]
pub struct EngineBuilder {
    config: Option<wasmtime::Config>,
    use_pooling_allocator: Option<bool>,
    max_instances: Option<u32>,
    compilation_cache_size: Option<u64>,
    compilation_cache_ttl: Option<Duration>,
    fuel_consumption: bool,
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
    pub fn with_max_instances(mut self, max: u32) -> Self {
        self.max_instances = Some(max);
        self
    }

    /// Enables or disables fuel consumption for the engine.
    pub fn with_fuel_consumption(mut self, enable: bool) -> Self {
        self.fuel_consumption = enable;
        self
    }

    /// Sets a custom wasmtime configuration for the engine.
    ///
    /// This allows full control over the wasmtime engine configuration,
    /// including compilation settings, runtime limits, and feature flags.
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
        // If a custom config was provided, use it as-is
        let config = if let Some(cfg) = self.config.take() {
            if self.max_instances.is_some() || self.use_pooling_allocator.is_some() {
                bail!(
                    "cannot use with_config() together with with_max_instances() or with_pooling_allocator()"
                );
            }
            cfg
        } else {
            let mut cfg = wasmtime::Config::default();
            // Async support must be enabled
            cfg.async_support(true);

            // The pooling allocator can be more efficient for workloads with many short-lived instances
            if let Ok(true) = use_pooling_allocator_by_default(self.use_pooling_allocator) {
                tracing::debug!("using pooling allocator by default");
                cfg.allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(
                    new_pooling_config(self.max_instances.unwrap_or(1000)),
                ));
            }

            cfg.consume_fuel(self.fuel_consumption);

            cfg
        };

        let inner = wasmtime::Engine::new(&config)?;
        let cache = Cache::builder()
            .max_capacity(self.compilation_cache_size.unwrap_or(100))
            .time_to_idle(
                self.compilation_cache_ttl
                    .unwrap_or(Duration::from_secs(600)),
            )
            .build();
        Ok(Engine { inner, cache })
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

pub fn imports_wasi_http(component: &Component) -> bool {
    let ty: wasmtime::component::types::Component = component.component_type();
    let engine = component.engine();

    ty.imports(engine)
        .any(|(import, _item)| import.starts_with("wasi:http"))
}

// TL;DR this is likely best for machines that can handle the large virtual memory requirement of the pooling allocator
// https://github.com/bytecodealliance/wasmtime/blob/b943666650696f1eb7ff8b217762b58d5ef5779d/src/commands/serve.rs#L641-L656
fn use_pooling_allocator_by_default(runtime_preference: Option<bool>) -> anyhow::Result<bool> {
    if let Some(v) = runtime_preference {
        return Ok(v);
    }

    if let Some(v) = getenv("WASMTIME_POOLING") {
        return Ok(v);
    }

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
