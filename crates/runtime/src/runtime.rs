use crate::{experimental::Features, ComponentConfig};

use core::fmt;
use core::fmt::Debug;
use core::time::Duration;

use std::{iter::zip, pin::pin, sync::Arc, thread, time::Instant};

use anyhow::{bail, Context};
use bytes::BytesMut;
use futures::future::try_join_all;
use tokio::{io::AsyncWriteExt as _, try_join};
use tracing::{debug, error, instrument, trace, warn, Instrument as _, Span};
use wasmtime::{
    component::{
        types::{self, Field},
        LinkerInstance, ResourceType, Type, Val,
    },
    AsContextMut as _, Engine, InstanceAllocationStrategy, PoolingAllocationConfig,
};
use wasmtime_wasi::WasiView;
use wit_bindgen_wrpc::tokio_util::codec::Encoder;
use wrpc_runtime_wasmtime::{read_value, RemoteResource, ValEncoder, WrpcView};
use wrpc_transport::{Index as _, Invoke, InvokeExt as _};

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
    rpc_timeout: Duration,
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
            rpc_timeout: Duration::from_secs(2),
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

    /// Sets the timeout for a component invoking an RPC call. Defaults to 2 seconds.
    /// This operates on second precision and value of 1 second is the minimum.
    /// Any value below 1 second will be interpreted as 1 second limit.
    #[must_use]
    pub fn rpc_timeout(self, rpc_timeout: Duration) -> Self {
        Self {
            rpc_timeout: rpc_timeout.max(Duration::from_secs(1)),
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
        let max_core_instances_per_component = 30;
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
                rpc_timeout: self.rpc_timeout,
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
    pub(crate) rpc_timeout: Duration,
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

/// Checks to see if a `ComponentFunc` is a function that should be linked with error handling
pub(crate) fn returns_rpc_error_result(item: &wasmtime::component::types::ComponentFunc) -> bool {
    let mut fn_results = item.results();
    // Returns true if the return type of the function is result<T, wasmcloud:bus/lattice.rpc-error>
    match fn_results.next() {
        Some(Type::Result(result_ty)) if fn_results.len() == 0 => match result_ty.err() {
            Some(Type::Record(record)) if record.fields().len() == 1 => {
                if let Some(Field {
                    ty: Type::Variant(_),
                    name: "wasmcloud-error",
                }) = record.fields().next()
                {
                    return true;
                }
            }
            _ => {}
        },
        _ => {}
    }
    false
}

// Copied directly from <https://docs.rs/wrpc-runtime-wasmtime/0.25.0/wrpc_runtime_wasmtime/fn.link_item.html>
// and modified to check ComponentFunc for error handling
/// Polyfill [`types::ComponentItem`] in a [`LinkerInstance`] using [`wrpc_transport::Invoke`]
#[instrument(level = "trace", skip_all)]
pub(crate) fn link_item<V>(
    engine: &Engine,
    linker: &mut LinkerInstance<V>,
    resources: impl Into<Arc<[ResourceType]>>,
    ty: types::ComponentItem,
    instance: impl Into<Arc<str>>,
    name: impl Into<Arc<str>>,
    cx: <V::Invoke as Invoke>::Context,
) -> wasmtime::Result<()>
where
    V: WasiView + WrpcView,
    <V::Invoke as Invoke>::Context: Clone + 'static,
{
    let instance = instance.into();
    let resources = resources.into();
    match ty {
        types::ComponentItem::ComponentFunc(ty) => {
            let name = name.into();
            if returns_rpc_error_result(&ty) {
                debug!(
                    ?instance,
                    ?name,
                    "linking function with wasmcloud error handling"
                );
                crate::runtime::link_function_with_error_handling(
                    linker, resources, ty, instance, name, cx,
                )?;
            } else {
                debug!(?instance, ?name, "linking function");
                wrpc_runtime_wasmtime::link_function(linker, resources, ty, instance, name, cx)?;
            }
        }
        types::ComponentItem::CoreFunc(_) => {
            bail!("polyfilling core functions not supported yet")
        }
        types::ComponentItem::Module(_) => bail!("polyfilling modules not supported yet"),
        types::ComponentItem::Component(ty) => {
            for (name, ty) in ty.imports(engine) {
                debug!(?instance, name, "linking component item");
                crate::runtime::link_item(
                    engine,
                    linker,
                    Arc::clone(&resources),
                    ty,
                    "",
                    name,
                    cx.clone(),
                )?;
            }
        }
        types::ComponentItem::ComponentInstance(ty) => {
            let name = name.into();
            let mut linker = linker
                .instance(&name)
                .with_context(|| format!("failed to instantiate `{name}` in the linker"))?;
            debug!(?instance, ?name, "linking instance");
            crate::runtime::link_instance(engine, &mut linker, resources, ty, name, cx)?;
        }
        types::ComponentItem::Type(_) => {}
        types::ComponentItem::Resource(_) => {
            let name = name.into();
            debug!(?instance, ?name, "linking resource");
            linker.resource(&name, ResourceType::host::<RemoteResource>(), |_, _| Ok(()))?;
        }
    }
    Ok(())
}

// Copied directly from <https://docs.rs/wrpc-runtime-wasmtime/0.25.0/wrpc_runtime_wasmtime/fn.link_instance.html>
// and modified to recurse here to check functions for errors instead of in the caller
/// Polyfill [`types::ComponentInstance`] in a [`LinkerInstance`] using [`wrpc_transport::Invoke`]
#[instrument(level = "trace", skip_all)]
pub(crate) fn link_instance<V>(
    engine: &Engine,
    linker: &mut LinkerInstance<V>,
    resources: impl Into<Arc<[ResourceType]>>,
    ty: types::ComponentInstance,
    name: impl Into<Arc<str>>,
    cx: <V::Invoke as Invoke>::Context,
) -> wasmtime::Result<()>
where
    V: WrpcView + WasiView,
    <V::Invoke as Invoke>::Context: Clone + 'static,
{
    let instance = name.into();
    let resources = resources.into();
    for (name, ty) in ty.exports(engine) {
        debug!(name, "linking instance item");
        crate::runtime::link_item(
            engine,
            linker,
            Arc::clone(&resources),
            ty,
            Arc::clone(&instance),
            name,
            cx.clone(),
        )?;
    }
    Ok(())
}

// Copied directly from <https://docs.rs/wrpc-runtime-wasmtime/0.25.0/wrpc_runtime_wasmtime/fn.link_function.html>
// and modified to handle errors
/// Polyfill [`types::ComponentFunc`] in a [`LinkerInstance`] using [`wrpc_transport::Invoke`]
#[instrument(level = "trace", skip_all)]
fn link_function_with_error_handling<V>(
    linker: &mut LinkerInstance<V>,
    resources: impl Into<Arc<[ResourceType]>>,
    ty: wasmtime::component::types::ComponentFunc,
    instance: impl Into<Arc<str>>,
    name: impl Into<Arc<str>>,
    cx: <V::Invoke as Invoke>::Context,
) -> wasmtime::Result<()>
where
    V: WrpcView + WasiView,
    <V::Invoke as Invoke>::Context: Clone + 'static,
{
    let span = Span::current();
    let instance = instance.into();
    let name = name.into();
    let resources = resources.into();
    linker.func_new_async(&Arc::clone(&name), move |mut store, params, results| {
        let cx = cx.clone();
        let ty = ty.clone();
        let instance = Arc::clone(&instance);
        let name = Arc::clone(&name);
        let resources = Arc::clone(&resources);
        Box::new(
            async move {
                let mut buf = BytesMut::default();
                let mut deferred = vec![];

                for (v, ref ty) in zip(params, ty.params()) {
                    let mut enc = ValEncoder::new(store.as_context_mut(), ty, &resources);
                    enc.encode(v, &mut buf)
                        .context("failed to encode parameter")?;
                    deferred.push(enc.deferred);
                }
                let clt = store.data().client();
                let timeout = store.data().timeout();
                let buf = buf.freeze();
                // TODO: set paths
                let paths = &[[]; 0];
                let rpc_name = rpc_func_name(&name);
                let start = Instant::now();
                // MODIFICATION FOR ERROR HANDLING BEGINS
                let invoke_res = if let Some(timeout) = timeout {
                    clt.timeout(timeout)
                        .invoke(cx, &instance, rpc_name, buf, paths)
                        .await
                } else {
                    clt.invoke(cx, &instance, rpc_name, buf, paths).await
                };

                let (outgoing, incoming) = match invoke_res {
                    Ok((outgoing, incoming)) => (outgoing, incoming),
                    Err(e) => {
                        error!(%e, %instance, %name, "failed to invoke polyfill via wRPC");
                        if let Some(v) = results.first_mut() {
                            *v = Val::Result(Err(Some(Box::new(Val::Record(vec![(
                                "wasmcloud-error".to_string(),
                                Val::Variant(
                                    "wasmcloud".to_string(),
                                    Some(Box::new(Val::String(e.to_string()))),
                                ),
                            )])))));
                        } else {
                            warn!(%instance, %name, "no results to write error to");
                        }
                        return Ok(());
                    }
                };
                let tx = async {
                    if let Err(e) = try_join_all(
                        zip(0.., deferred)
                            .filter_map(|(i, f)| f.map(|f| (outgoing.index(&[i]), f)))
                            .map(|(w, f)| async move {
                                let w = w?;
                                f(w).await
                            }),
                    )
                    .await
                    .context("failed to write asynchronous parameters")
                    {
                        error!(%e, %instance, %name, "failed to write asynchronous parameters");
                        return Ok(vec![Val::Result(Err(Some(Box::new(Val::Record(vec![(
                            "wasmcloud-error".to_string(),
                            Val::Variant(
                                "wrpc".to_string(),
                                Some(Box::new(Val::String(e.to_string()))),
                            ),
                        )])))))]);
                    };
                    let mut outgoing = pin!(outgoing);
                    if let Err(e) = outgoing.flush().await{
                    error!(%e, %instance, %name, "failed to flush outgoing stream");
                        Ok(vec![Val::Result(Err(Some(Box::new(Val::Record(vec![(
                            "wasmcloud-error".to_string(),
                            Val::Variant(
                                "wrpc".to_string(),
                                Some(Box::new(Val::String(e.to_string()))),
                            ),
                        )])))))])
                    } else {
                        if let Err(err) = outgoing.shutdown().await {
                            trace!(?err, "failed to shutdown outgoing stream");
                        }
                        anyhow::Ok(vec![])
                    }
                };
                let rx = async {
                    let mut incoming = pin!(incoming);

                    let ty_results = ty.results();
                    let mut rx_results: Vec<Val> = vec![Val::Bool(false); ty_results.len()];
                    for (i, (v, ref ty)) in zip(rx_results.as_mut_slice(), ty_results).enumerate()
                    {
                        read_value(&mut store, &mut incoming, &resources, v, ty, &[i])
                            .await
                            .with_context(|| format!("failed to decode return value {i}"))?;
                    }
                    Ok(rx_results)
                };
                // TODO(brooks): would be great to find a way to not duplicate the matches for timeouts
                if let Some(timeout) = timeout {
                    let timeout =
                        timeout.saturating_sub(Instant::now().saturating_duration_since(start));
                    match try_join!(
                        async {
                            tokio::time::timeout(timeout, tx)
                                .await
                                .context("data transmission timed out")
                        },
                        async {
                            tokio::time::timeout(timeout, rx)
                                .await
                                .context("data receipt timed out")
                        },
                    ) {
                        // Any errors in transmission are reported here
                        Ok((Ok(tx), Ok(_))) if !tx.is_empty() => {
                            for (actual, result) in zip(tx, results) {
                                *result = actual;
                            }
                        }
                        // Write any received values to the results
                        Ok((Ok(_), Ok(rx)))  => {
                            for (actual, result) in zip(rx, results) {
                                *result = actual;
                            }
                        }
                        Ok((Err(e), _) | (_, Err(e))) => {
                            error!(%e, %instance, %name, "failed to transmit or receive data");
                            if let Some(v) = results.first_mut() {
                                *v = Val::Result(Err(Some(Box::new(Val::Record(vec![(
                                    "wasmcloud-error".to_string(),
                                    Val::Variant(
                                        "wrpc".to_string(),
                                        Some(Box::new(Val::String(e.to_string()))),
                                    ),
                                )])))));
                            }
                        }
                        Err(e) => {
                            error!(%e, %instance, %name, "failed to transmit or receive data due to timeout");
                            if let Some(v) = results.first_mut() {
                                *v = Val::Result(Err(Some(Box::new(Val::Record(vec![(
                                    "wasmcloud-error".to_string(),
                                    Val::Variant(
                                        "wrpc".to_string(),
                                        Some(Box::new(Val::String(e.to_string()))),
                                    ),
                                )])))));
                            } else {
                                warn!(%instance, %name, "no results to write timeout error to");
                            }
                        }
                    }
                } else {
                    match try_join!(tx, rx) {
                        // Any errors in transmission are reported here
                        Ok((tx, _)) if !tx.is_empty() => {
                            for (actual, result) in zip(tx, results) {
                                *result = actual;
                            }
                        }
                        // Write any received values to the results
                        Ok((_, rx)) => {
                            for (actual, result) in zip(rx, results) {
                                *result = actual;
                            }
                        }
                        Err(e) => {
                            error!(%e, %instance, %name, "failed to transmit or receive data");
                            if let Some(v) = results.first_mut() {
                                *v = Val::Result(Err(Some(Box::new(Val::Record(vec![(
                                    "wasmcloud-error".to_string(),
                                    Val::Variant(
                                        "wrpc".to_string(),
                                        Some(Box::new(Val::String(e.to_string()))),
                                    ),
                                )])))));
                            } else {
                                warn!(%instance, %name, "no results to write error to");
                            }
                        }
                    }
                }
                // MODIFICATION FOR ERROR HANDLING ENDS
                Ok(())
            }
            .instrument(span.clone()),
        )
    })
}

// this returns the RPC name for a wasmtime function name.
// Unfortunately, the [`types::ComponentFunc`] does not include the kind information and we want to
// avoid (re-)parsing the WIT here.
fn rpc_func_name(name: &str) -> &str {
    if let Some(name) = name.strip_prefix("[constructor]") {
        name
    } else if let Some(name) = name.strip_prefix("[static]") {
        name
    } else if let Some(name) = name.strip_prefix("[method]") {
        name
    } else {
        name
    }
}
