mod wasmbus;

use wasmbus::guest_call;

use crate::actor::claims;
use crate::capability::{host, logging, HandlerBuilder};
use crate::Runtime;

use core::fmt::{self, Debug};

use anyhow::{ensure, Context};
use tracing::{instrument, trace, warn};
use wascap::jwt;
use wasmtime_wasi::WasiCtxBuilder;

/// Actor module instance configuration
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// Minimum amount of WebAssembly memory pages to allocate for WebAssembly module instance.
    ///
    /// A WebAssembly memory page size is 64k.
    pub min_memory_pages: u32,
    /// WebAssembly memory page allocation limit for a WebAssembly module instance.
    ///
    /// A WebAssembly memory page size is 64k.
    pub max_memory_pages: Option<u32>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            min_memory_pages: 4,
            max_memory_pages: None,
        }
    }
}

struct Ctx {
    wasi: wasmtime_wasi::WasiCtx,
    wasmbus: wasmbus::Ctx,
}

impl Debug for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("runtime", &"wasmtime")
            .field("wasmbus", &self.wasmbus)
            .finish()
    }
}

impl Ctx {
    fn reset(&mut self) {
        self.wasmbus.reset();
    }
}

/// Pre-compiled actor [Module], which is cheapily-[Cloneable](Clone)
#[derive(Clone)]
pub struct Module {
    claims: jwt::Claims<jwt::Actor>,
    config: Config,
    handler: HandlerBuilder,
    module: wasmtime::Module,
}

impl Debug for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Module")
            .field("claims", &self.claims)
            .field("config", &self.config)
            .field("handler", &self.handler)
            .field("runtime", &"wasmtime")
            .finish()
    }
}

impl Module {
    /// [Claims](jwt::Claims) associated with this [Module].
    #[instrument]
    pub fn claims(&self) -> &jwt::Claims<jwt::Actor> {
        &self.claims
    }
}

impl Module {
    /// Extracts [Claims](jwt::Claims) from WebAssembly module and compiles it using [Runtime].
    #[instrument(skip(wasm))]
    pub fn new(rt: &Runtime, wasm: impl AsRef<[u8]>) -> anyhow::Result<Self> {
        let wasm = wasm.as_ref();
        let claims = claims(wasm)?;
        let module = wasmtime::Module::new(&rt.engine, wasm).context("failed to compile module")?;
        Ok(Self {
            module,
            claims,
            handler: rt.handler.clone(),
            config: rt.module_config,
        })
    }

    /// Returns an [ConfiguredModule], which can be used to configure and produce an [Instance].
    #[instrument]
    pub fn configure(&self) -> ConfiguredModule {
        self.into()
    }

    /// Like [Self::configure], but moves the [Module].
    #[instrument]
    pub fn into_configure(self) -> ConfiguredModule {
        self.into()
    }

    /// Like [Self::configure], but moves the [Module] and returns the associated [jwt::Claims].
    #[instrument]
    pub fn into_configure_claims(self) -> (ConfiguredModule, jwt::Claims<jwt::Actor>) {
        self.into()
    }

    /// Instantiates a [Module] and returns the resulting [Instance].
    #[instrument]
    pub async fn instantiate(&self) -> anyhow::Result<Instance> {
        self.configure().instantiate().await
    }

    /// Instantiate a [Module] producing an [Instance] and invoke an operation on it using [Instance::call]
    #[instrument(skip(operation, payload))]
    pub async fn call(
        &self,
        operation: impl AsRef<str>,
        payload: impl Into<Vec<u8>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        self.configure().call(operation, payload).await
    }
}

/// A component paired with configuration
pub struct ConfiguredModule {
    module: wasmtime::Module,
    config: Config,
    handler: HandlerBuilder,
    wasi: WasiCtxBuilder,
}

impl Debug for ConfiguredModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfiguredModule")
            .field("handler", &self.handler)
            .field("runtime", &"wasmtime")
            .finish()
    }
}

impl ConfiguredModule {
    /// Set a [`host::Host`] handler to use for this instance
    #[must_use]
    pub fn host(self, host: impl host::Host + Sync + Send + 'static) -> Self {
        Self {
            handler: self.handler.host(host),
            ..self
        }
    }

    /// Set a [`logging::Host`] handler to use for this instance
    #[must_use]
    pub fn logging(self, logging: impl logging::Host + Sync + Send + 'static) -> Self {
        Self {
            handler: self.handler.logging(logging),
            ..self
        }
    }

    /// Configure module to inherit standard output of the process
    #[must_use]
    pub fn inherit_stdout(self) -> Self {
        Self {
            wasi: self.wasi.inherit_stdout(),
            ..self
        }
    }

    /// Configure module to inherit standard error of the process
    #[must_use]
    pub fn inherit_stderr(self) -> Self {
        Self {
            wasi: self.wasi.inherit_stderr(),
            ..self
        }
    }

    /// Instantiates a [ConfiguredModule] and returns the resulting [Instance].
    #[instrument]
    pub async fn instantiate(self) -> anyhow::Result<Instance> {
        // TODO: Set stdio pipes
        let wasi = self
            .wasi
            .arg("main.wasm")
            .context("failed to set argv[0]")?
            .build();
        let ctx = Ctx {
            wasi,
            wasmbus: wasmbus::Ctx::new(self.handler),
        };

        let engine = self.module.engine();

        let mut store = wasmtime::Store::new(engine, ctx);
        let mut linker = wasmtime::Linker::<Ctx>::new(engine);

        wasmtime_wasi::add_to_linker(&mut linker, |ctx| &mut ctx.wasi)
            .context("failed to link WASI")?;
        wasmbus::add_to_linker(&mut linker).context("failed to link wasmbus")?;

        let memory = wasmtime::Memory::new(
            &mut store,
            wasmtime::MemoryType::new(self.config.min_memory_pages, self.config.max_memory_pages),
        )
        .context("failed to initialize memory")?;
        linker
            .define_name(&store, "memory", memory)
            .context("failed to define `memory`")?;

        let instance = linker
            .instantiate_async(&mut store, &self.module)
            .await
            .context("failed to instantiate module")?;

        if let Ok(start) = instance.get_typed_func(&mut store, "_start") {
            start
                .call_async(&mut store, ())
                .await
                .context("failed to call `_start`")?;
        }
        Ok(Instance { instance, store })
    }

    /// Instantiates a [ConfiguredModule] producing an [Instance] and invokes an operation on it using [Instance::call]
    #[instrument(skip(operation, payload))]
    pub async fn call(
        self,
        operation: impl AsRef<str>,
        payload: impl Into<Vec<u8>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        let operation = operation.as_ref();
        self.instantiate()
            .await
            .context("failed to instantiate module")?
            .call(operation, payload)
            .await
            .with_context(|| format!("failed to call operation `{operation}` on module"))
    }
}

impl From<Module> for ConfiguredModule {
    fn from(
        Module {
            module,
            config,
            handler,
            ..
        }: Module,
    ) -> Self {
        Self {
            module,
            handler,
            config,
            wasi: WasiCtxBuilder::new(),
        }
    }
}

impl From<Module> for (ConfiguredModule, jwt::Claims<jwt::Actor>) {
    fn from(
        Module {
            module,
            config,
            handler,
            claims,
        }: Module,
    ) -> Self {
        (
            ConfiguredModule {
                module,
                handler,
                config,
                wasi: WasiCtxBuilder::new(),
            },
            claims,
        )
    }
}

impl From<&Module> for ConfiguredModule {
    fn from(
        Module {
            module,
            config,
            handler,
            ..
        }: &Module,
    ) -> Self {
        Self {
            module: module.clone(),
            handler: handler.clone(),
            config: *config,
            wasi: WasiCtxBuilder::new(),
        }
    }
}

/// An instance of a [Module]
pub struct Instance {
    instance: wasmtime::Instance,
    store: wasmtime::Store<Ctx>,
}

impl Debug for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Instance")
            .field("runtime", &"wasmtime")
            .finish()
    }
}

impl Instance {
    /// Invoke an operation on an [Instance].
    #[instrument(skip_all)]
    pub async fn call(
        &mut self,
        operation: impl Into<String>,
        payload: impl Into<Vec<u8>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        self.store.data_mut().reset();

        let operation = operation.into();
        let operation_len = operation
            .len()
            .try_into()
            .context("operation string length does not fit in u32")?;

        let payload = payload.into();
        let payload_len = payload
            .len()
            .try_into()
            .context("payload length does not fit in u32")?;

        let func: wasmtime::TypedFunc<guest_call::Params, guest_call::Result> = self
            .instance
            .get_typed_func(&mut self.store, "__guest_call")
            .context("failed to get `__guest_call` export")?;

        self.store
            .data_mut()
            .wasmbus
            .set_guest_call(operation, payload);

        let code = func
            .call_async(&mut self.store, (operation_len, payload_len))
            .await
            .context("failed to call `__guest_call`")?;
        if let Some(err) = self.store.data_mut().wasmbus.take_guest_error() {
            return Ok(Err(err));
        } else if let Some(err) = self.store.data_mut().wasmbus.take_host_error() {
            return Ok(Err(err));
        }
        let response = self.store.data_mut().wasmbus.take_guest_response();
        let console_log: Vec<_> = self.store.data_mut().wasmbus.take_console_log();
        ensure!(code == 1, "operation failed with exit code `{code}`");
        if !console_log.is_empty() {
            trace!(?console_log);
        }
        Ok(Ok(response))
    }
}
