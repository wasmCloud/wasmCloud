use crate::ComponentConfig;

use core::fmt;
use core::fmt::Debug;
use core::str::FromStr;
use core::time::Duration;

use std::env::{self, VarError};
use std::thread;

use anyhow::Context as _;
use tracing::{debug, error};
use wasmtime::{InstanceAllocationStrategy, PoolingAllocationConfig};

fn getenv<T>(key: &str) -> Option<T>
where
    T: FromStr,
    T::Err: Debug,
{
    match env::var(key).as_deref().map(FromStr::from_str) {
        Ok(Ok(v)) => Some(v),
        Ok(Err(err)) => {
            error!(?err, "failed to parse `{key}` value, ignoring");
            None
        }
        Err(VarError::NotPresent) => None,
        Err(VarError::NotUnicode(..)) => {
            error!("`{key}` value is not valid UTF-8, ignoring");
            None
        }
    }
}

fn new_pooling_config() -> PoolingAllocationConfig {
    let mut config = PoolingAllocationConfig::default();
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_MAX_UNUSED_WASM_SLOTS") {
        config.max_unused_warm_slots(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_DECOMMIT_BATCH_SIZE") {
        config.decommit_batch_size(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_ASYNC_STACK_ZEROING") {
        config.async_stack_zeroing(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_ASYNC_STACK_KEEP_RESIDENT") {
        config.async_stack_keep_resident(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_LINEAR_MEMORY_KEEP_RESIDENT") {
        config.linear_memory_keep_resident(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_TABLE_KEEP_RESIDENT") {
        config.table_keep_resident(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_TOTAL_COMPONENT_INSTANCES") {
        config.total_component_instances(v);
    } else {
        config.total_component_instances(10_000);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_MAX_COMPONENT_INSTANCE_SIZE") {
        config.max_component_instance_size(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_MAX_CORE_INSTANCES_PER_COMPONENT") {
        config.max_core_instances_per_component(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_MAX_MEMORIES_PER_COMPONENT") {
        config.max_memories_per_component(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_MAX_TABLES_PER_COMPONENT") {
        config.max_tables_per_component(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_TOTAL_MEMORIES") {
        config.total_memories(v);
    } else {
        config.total_memories(10_000);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_TOTAL_TABLES") {
        config.total_tables(v);
    } else {
        config.total_tables(10_000);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_TOTAL_STACKS") {
        config.total_stacks(v);
    } else {
        config.total_stacks(10_000);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_TOTAL_CORE_INSTANCES") {
        config.total_core_instances(v);
    } else {
        config.total_core_instances(10_000);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_MAX_CORE_INSTANCE_SIZE") {
        config.max_core_instance_size(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_MAX_TABLES_PER_MODULE") {
        config.max_tables_per_module(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_TABLE_ELEMENTS") {
        config.table_elements(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_MAX_MEMORIES_PER_MODULE") {
        config.max_memories_per_module(v);
    }
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_MAX_MEMORY_SIZE") {
        config.max_memory_size(v);
    }
    // TODO: Add memory protection key support
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING_TOTAL_GC_HEAPS") {
        config.total_gc_heaps(v);
    } else {
        config.total_gc_heaps(10_000);
    }
    config
}

// https://github.com/bytecodealliance/wasmtime/blob/b943666650696f1eb7ff8b217762b58d5ef5779d/src/commands/serve.rs#L641-L656
fn use_pooling_allocator_by_default() -> anyhow::Result<bool> {
    const BITS_TO_TEST: u32 = 42;
    if let Some(v) = getenv("WASMCLOUD_WASMTIME_POOLING") {
        return Ok(v);
    }
    let mut config = wasmtime::Config::new();
    config.wasm_memory64(true);
    config.static_memory_maximum_size(1 << BITS_TO_TEST);
    let engine = wasmtime::Engine::new(&config)?;
    let mut store = wasmtime::Store::new(&engine, ());
    // NB: the maximum size is in wasm pages to take out the 16-bits of wasm
    // page size here from the maximum size.
    let ty = wasmtime::MemoryType::new64(0, Some(1 << (BITS_TO_TEST - 16)));
    Ok(wasmtime::Memory::new(&mut store, ty).is_ok())
}

/// [`RuntimeBuilder`] used to configure and build a [Runtime]
#[derive(Clone)]
pub struct RuntimeBuilder {
    max_execution_time: Duration,
    component_config: ComponentConfig,
}

impl Default for RuntimeBuilder {
    fn default() -> Self {
        Self {
            max_execution_time: Duration::from_secs(10 * 60),
            component_config: ComponentConfig::default(),
        }
    }
}

impl RuntimeBuilder {
    /// Returns a new [`RuntimeBuilder`]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a custom [`ComponentConfig`] to use for all component instances
    #[must_use]
    pub fn component_config(self, component_config: ComponentConfig) -> Self {
        Self {
            component_config,
            ..self
        }
    }

    /// Sets the maximum execution time of a component. Defaults to 10 minutes.
    /// This operates on second precision and value of 1 second is the minimum.
    /// Any value below 1 second will be interpreted as 1 second limit.
    #[must_use]
    pub fn max_execution_time(self, max_execution_time: Duration) -> Self {
        Self {
            max_execution_time: max_execution_time.max(Duration::from_secs(1)),
            ..self
        }
    }

    /// Turns this builder into a [`Runtime`]
    ///
    /// # Errors
    ///
    /// Fails if the configuration is not valid
    pub fn build(self) -> anyhow::Result<(Runtime, thread::JoinHandle<Result<(), ()>>)> {
        let mut engine_config = wasmtime::Config::default();
        engine_config.async_support(true);
        engine_config.epoch_interruption(true);
        engine_config.wasm_component_model(true);
        if let Ok(true) = use_pooling_allocator_by_default() {
            debug!("using pooling allocator");
            engine_config
                .allocation_strategy(InstanceAllocationStrategy::Pooling(new_pooling_config()));
        } else {
            debug!("using on-demand allocator");
            engine_config.allocation_strategy(InstanceAllocationStrategy::OnDemand);
        }
        if let Some(v) = getenv("WASMCLOUD_WASMTIME_DEBUG_INFO") {
            engine_config.debug_info(v);
        }
        if let Some(v) = getenv("WASMCLOUD_WASMTIME_MAX_WASM_STACK") {
            engine_config.max_wasm_stack(v);
        }
        if let Some(v) = getenv("WASMCLOUD_WASMTIME_ASYNC_STACK_SIZE") {
            engine_config.async_stack_size(v);
        }
        let engine = match wasmtime::Engine::new(&engine_config)
            .context("failed to construct engine")
        {
            Ok(engine) => engine,
            Err(err) => {
                error!(?err, "failed to construct engine with pooling allocator, falling back to dynamic allocator which may result in slower startup and execution of components");
                engine_config.allocation_strategy(InstanceAllocationStrategy::OnDemand);
                wasmtime::Engine::new(&engine_config).context("failed to construct engine")?
            }
        };
        let epoch = {
            let engine = engine.weak();
            thread::spawn(move || loop {
                thread::sleep(Duration::from_secs(1));
                let Some(engine) = engine.upgrade() else {
                    return Ok(());
                };
                engine.increment_epoch();
            })
        };
        Ok((
            Runtime {
                engine,
                component_config: self.component_config,
                max_execution_time: self.max_execution_time,
            },
            epoch,
        ))
    }
}

impl TryFrom<RuntimeBuilder> for (Runtime, thread::JoinHandle<Result<(), ()>>) {
    type Error = anyhow::Error;

    fn try_from(builder: RuntimeBuilder) -> Result<Self, Self::Error> {
        builder.build()
    }
}

/// Shared wasmCloud runtime
#[derive(Clone)]
pub struct Runtime {
    pub(crate) engine: wasmtime::Engine,
    pub(crate) component_config: ComponentConfig,
    pub(crate) max_execution_time: Duration,
}

impl Debug for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runtime")
            .field("component_config", &self.component_config)
            .field("runtime", &"wasmtime")
            .field("max_execution_time", &"max_execution_time")
            .finish_non_exhaustive()
    }
}

impl Runtime {
    /// Returns a new [`Runtime`] configured with defaults
    ///
    /// # Errors
    ///
    /// Returns an error if the default configuration is invalid
    #[allow(clippy::type_complexity)]
    pub fn new() -> anyhow::Result<(Self, thread::JoinHandle<Result<(), ()>>)> {
        Self::builder().try_into()
    }

    /// Returns a new [`RuntimeBuilder`], which can be used to configure and build a [Runtime]
    #[must_use]
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    /// [Runtime] version
    #[must_use]
    pub fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }
}
