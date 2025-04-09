use crate::{experimental::Features, ComponentConfig};

use core::fmt;
use core::fmt::Debug;
use core::time::Duration;

use std::thread;

use anyhow::Context;
use wasmtime::{InstanceAllocationStrategy, PoolingAllocationConfig};

/// Default max linear memory for a component (256 MiB)
pub const MAX_LINEAR_MEMORY: u64 = 256 * 1024 * 1024;
/// Default max component size (50 MiB)
pub const MAX_COMPONENT_SIZE: u64 = 50 * 1024 * 1024;
/// Default max number of components
pub const MAX_COMPONENTS: u32 = 10_000;

/// [`RuntimeBuilder`] used to configure and build a [Runtime]
#[derive(Clone, Default)]
pub struct RuntimeBuilder {
    engine_config: wasmtime::Config,
    max_components: u32,
    max_component_size: u64,
    max_linear_memory: u64,
    max_execution_time: Duration,
    component_config: ComponentConfig,
    force_pooling_allocator: bool,
    experimental_features: Features,
}

impl RuntimeBuilder {
    /// Returns a new [`RuntimeBuilder`]
    #[must_use]
    pub fn new() -> Self {
        let mut engine_config = wasmtime::Config::default();
        engine_config.async_support(true);
        engine_config.epoch_interruption(true);
        engine_config.wasm_component_model(true);

        Self {
            engine_config,
            max_components: MAX_COMPONENTS,
            // Why so large you ask? Well, python components are chonky, like 35MB for a hello world
            // chonky. So this is pretty big for now.
            max_component_size: MAX_COMPONENT_SIZE,
            max_linear_memory: MAX_LINEAR_MEMORY,
            max_execution_time: Duration::from_secs(10 * 60),
            component_config: ComponentConfig::default(),
            force_pooling_allocator: false,
            experimental_features: Features::default(),
        }
    }

    /// Set a custom [`ComponentConfig`] to use for all component instances
    #[must_use]
    pub fn component_config(self, component_config: ComponentConfig) -> Self {
        Self {
            component_config,
            ..self
        }
    }

    /// Sets the maximum number of components that can be run simultaneously. Defaults to 10000
    #[must_use]
    pub fn max_components(self, max_components: u32) -> Self {
        Self {
            max_components,
            ..self
        }
    }

    /// Sets the maximum size of a component instance, in bytes. Defaults to 50MB
    #[must_use]
    pub fn max_component_size(self, max_component_size: u64) -> Self {
        Self {
            max_component_size,
            ..self
        }
    }

    /// Sets the maximum amount of linear memory that can be used by all components. Defaults to 10MB
    #[must_use]
    pub fn max_linear_memory(self, max_linear_memory: u64) -> Self {
        Self {
            max_linear_memory,
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

    /// Forces the use of the pooling allocator. This may cause the runtime to fail if there isn't enough memory for the pooling allocator
    #[must_use]
    pub fn force_pooling_allocator(self) -> Self {
        Self {
            force_pooling_allocator: true,
            ..self
        }
    }

    /// Set the experimental features to enable in the runtime
    #[must_use]
    pub fn experimental_features(self, experimental_features: Features) -> Self {
        Self {
            experimental_features,
            ..self
        }
    }

    /// Turns this builder into a [`Runtime`]
    ///
    /// # Errors
    ///
    /// Fails if the configuration is not valid
    #[allow(clippy::type_complexity)]
    pub fn build(mut self) -> anyhow::Result<(Runtime, thread::JoinHandle<Result<(), ()>>)> {
        let mut pooling_config = PoolingAllocationConfig::default();

        // Right now we assume tables_per_component is the same as memories_per_component just like
        // the default settings (which has a 1:1 relationship between total memories and total
        // tables), but we may want to change that later. I would love to figure out a way to
        // configure all these values via something smarter that can look at total memory available
        let memories_per_component = 1;
        let tables_per_component = 1;
        let max_core_instances_per_component = 50;
        let table_elements = 15000;

        #[allow(clippy::cast_possible_truncation)]
        pooling_config
            .total_component_instances(self.max_components)
            .total_core_instances(self.max_components)
            .total_gc_heaps(self.max_components)
            .total_stacks(self.max_components)
            .max_component_instance_size(self.max_component_size as usize)
            .max_core_instances_per_component(max_core_instances_per_component)
            .max_tables_per_component(20)
            .table_elements(table_elements)
            // The number of memories an instance can have effectively limits the number of inner components
            // a composed component can have (since each inner component has its own memory). We default to 32 for now, and
            // we'll see how often this limit gets reached.
            .max_memories_per_component(max_core_instances_per_component * memories_per_component)
            .total_memories(self.max_components * memories_per_component)
            .total_tables(self.max_components * tables_per_component)
            // Restrict the maximum amount of linear memory that can be used by a component,
            // which influences two things we care about:
            //
            // - How large of a component we can load (i.e. all components must be less than this value)
            // - How much memory a fully loaded host carrying c components will use
            .max_memory_size(self.max_linear_memory as usize)
            // These numbers are set to avoid page faults when trying to claim new space on linux
            .linear_memory_keep_resident(10 * 1024)
            .table_keep_resident(10 * 1024);
        self.engine_config
            .allocation_strategy(InstanceAllocationStrategy::Pooling(pooling_config));
        let engine = match wasmtime::Engine::new(&self.engine_config)
            .context("failed to construct engine")
        {
            Ok(engine) => engine,
            Err(e) if self.force_pooling_allocator => {
                anyhow::bail!("failed to construct engine with pooling allocator: {}", e)
            }
            Err(e) => {
                tracing::warn!(err = %e, "failed to construct engine with pooling allocator, falling back to dynamic allocator which may result in slower startup and execution of components.");
                self.engine_config
                    .allocation_strategy(InstanceAllocationStrategy::OnDemand);
                wasmtime::Engine::new(&self.engine_config).context("failed to construct engine")?
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
                experimental_features: self.experimental_features,
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
    pub(crate) experimental_features: Features,
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
    pub fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// Returns a boolean indicating whether the runtime should skip linking a feature-gated instance
    pub(crate) fn skip_feature_gated_instance(&self, instance: &str) -> bool {
        matches!(
            instance,
            "wasmcloud:messaging/producer@0.3.0"
                | "wasmcloud:messaging/request-reply@0.3.0"
                | "wasmcloud:messaging/types@0.3.0"
                if self.experimental_features.wasmcloud_messaging_v3)
    }
}
