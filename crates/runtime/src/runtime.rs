use crate::capability::builtin::WrpcBus;
use crate::capability::{
    builtin, Blobstore, IncomingHttp, KeyValueAtomics, KeyValueStore, Logging, Messaging,
    OutgoingHttp,
};
use crate::ComponentConfig;

use core::fmt;
use core::fmt::Debug;
use core::time::Duration;

use std::sync::Arc;
use std::thread;

use anyhow::Context;
use builtin::Config;
use tokio::sync::oneshot;
use wasmtime::{InstanceAllocationStrategy, PoolingAllocationConfig};

const KB: u64 = 1024;
const MB: u64 = KB * 1024;
const GB: u64 = MB * 1024;

/// [`RuntimeBuilder`] used to configure and build a [Runtime]
#[derive(Clone, Default)]
pub struct RuntimeBuilder {
    engine_config: wasmtime::Config,
    max_components: u32,
    max_component_size: u64,
    max_execution_time: Duration,
    handler: builtin::HandlerBuilder,
    component_config: ComponentConfig,
    force_pooling_allocator: bool,
}

impl RuntimeBuilder {
    /// Returns a new [`RuntimeBuilder`]
    #[must_use]
    pub fn new() -> Self {
        let mut engine_config = wasmtime::Config::default();
        engine_config.async_support(true);
        engine_config.epoch_interruption(true);
        engine_config.memory_init_cow(false);
        engine_config.wasm_component_model(true);

        Self {
            engine_config,
            max_components: 10000,
            // Why so large you ask? Well, python components are chonky, like 35MB for a hello world
            // chonky. So this is pretty big for now.
            max_component_size: 50 * MB,
            max_execution_time: Duration::from_secs(10 * 60),
            handler: builtin::HandlerBuilder::default(),
            component_config: ComponentConfig::default(),
            force_pooling_allocator: false,
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

    /// Set a [`Blobstore`] handler to use for all component instances unless overriden for the instance
    #[must_use]
    pub fn blobstore(self, blobstore: Arc<impl Blobstore + Sync + Send + 'static>) -> Self {
        Self {
            handler: self.handler.blobstore(blobstore),
            ..self
        }
    }

    /// Set a [`Bus`] handler to use for all component instances unless overriden for the instance
    #[must_use]
    pub fn bus(self, bus: Arc<impl WrpcBus + Sync + Send + 'static>) -> Self {
        Self {
            handler: self.handler.bus(bus),
            ..self
        }
    }

    /// Set a [`Config`] handler to use for all component instances unless overriden for the instance
    #[must_use]
    pub fn config(self, config: Arc<impl Config + Sync + Send + 'static>) -> Self {
        Self {
            handler: self.handler.config(config),
            ..self
        }
    }

    /// Set a [`IncomingHttp`] handler to use for all component instances unless overriden for the instance
    #[must_use]
    pub fn incoming_http(
        self,
        incoming_http: Arc<impl IncomingHttp + Sync + Send + 'static>,
    ) -> Self {
        Self {
            handler: self.handler.incoming_http(incoming_http),
            ..self
        }
    }

    /// Set a [`KeyValueAtomics`] handler to use for all component instances unless overriden for the instance
    #[must_use]
    pub fn keyvalue_atomics(
        self,
        keyvalue_atomics: Arc<impl KeyValueAtomics + Sync + Send + 'static>,
    ) -> Self {
        Self {
            handler: self.handler.keyvalue_atomics(keyvalue_atomics),
            ..self
        }
    }

    /// Set a [`KeyValueStore`] handler to use for all component instances unless overriden for the instance
    #[must_use]
    pub fn keyvalue_store(
        self,
        keyvalue_store: Arc<impl KeyValueStore + Sync + Send + 'static>,
    ) -> Self {
        Self {
            handler: self.handler.keyvalue_store(keyvalue_store),
            ..self
        }
    }

    /// Set a [`Logging`] handler to use for all component instances unless overriden for the instance
    #[must_use]
    pub fn logging(self, logging: Arc<impl Logging + Sync + Send + 'static>) -> Self {
        Self {
            handler: self.handler.logging(logging),
            ..self
        }
    }

    /// Set a [`Messaging`] handler to use for all component instances unless overriden for the instance
    #[must_use]
    pub fn messaging(self, messaging: Arc<impl Messaging + Sync + Send + 'static>) -> Self {
        Self {
            handler: self.handler.messaging(messaging),
            ..self
        }
    }

    /// Set a [`OutgoingHttp`] handler to use for all component instances unless overriden for the instance
    #[must_use]
    pub fn outgoing_http(
        self,
        outgoing_http: Arc<impl OutgoingHttp + Sync + Send + 'static>,
    ) -> Self {
        Self {
            handler: self.handler.outgoing_http(outgoing_http),
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

    /// Turns this builder into a [`Runtime`]
    ///
    /// # Errors
    ///
    /// Fails if the configuration is not valid
    #[allow(clippy::type_complexity)]
    pub fn build(
        mut self,
    ) -> anyhow::Result<(
        Runtime,
        thread::JoinHandle<Result<(), ()>>,
        oneshot::Receiver<()>,
    )> {
        let mut pooling_config = PoolingAllocationConfig::default();

        // Right now we assume tables_per_component is the same as memories_per_component just like
        // the default settings (which has a 1:1 relationship between total memories and total
        // tables), but we may want to change that later. I would love to figure out a way to
        // configure all these values via something smarter that can look at total memory available
        let memories_per_component = 1;
        let tables_per_component = 1;
        let max_core_instances_per_component = 30;
        let table_elements = 15000;

        #[allow(clippy::cast_possible_truncation)]
        pooling_config
            .total_component_instances(self.max_components)
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
            // This means the max host memory any single component can take is 2 GB. This would be a
            // lot, so we shouldn't need to tweak this for a while. We can always expose this option
            // later
            .max_memory_size((2 * GB / (64 * KB)) as usize) //64 KB is the wasm page size
            // These numbers are set to avoid page faults when trying to claim new space on linux
            .linear_memory_keep_resident((10 * MB) as usize)
            .table_keep_resident((10 * MB) as usize);
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
        let (epoch_tx, epoch_rx) = oneshot::channel();
        let epoch = {
            let engine = engine.weak();
            thread::spawn(move || loop {
                thread::sleep(Duration::from_secs(1));
                let Some(engine) = engine.upgrade() else {
                    return epoch_tx.send(());
                };
                engine.increment_epoch();
            })
        };
        Ok((
            Runtime {
                engine,
                handler: self.handler,
                component_config: self.component_config,
                max_execution_time: self.max_execution_time,
            },
            epoch,
            epoch_rx,
        ))
    }
}

impl TryFrom<RuntimeBuilder>
    for (
        Runtime,
        thread::JoinHandle<Result<(), ()>>,
        oneshot::Receiver<()>,
    )
{
    type Error = anyhow::Error;

    fn try_from(builder: RuntimeBuilder) -> Result<Self, Self::Error> {
        builder.build()
    }
}

/// Shared wasmCloud runtime
#[derive(Clone)]
pub struct Runtime {
    pub(crate) engine: wasmtime::Engine,
    pub(crate) handler: builtin::HandlerBuilder,
    pub(crate) component_config: ComponentConfig,
    pub(crate) max_execution_time: Duration,
}

impl Debug for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runtime")
            .field("handler", &self.handler)
            .field("component_config", &self.component_config)
            .field("runtime", &"wasmtime")
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
    pub fn new() -> anyhow::Result<(
        Self,
        thread::JoinHandle<Result<(), ()>>,
        oneshot::Receiver<()>,
    )> {
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
