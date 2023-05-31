use crate::actor::claims;
use crate::capability::{host, logging, Handler, HandlerBuilder, Interfaces};
use crate::Runtime;

use core::fmt::{self, Debug};

use anyhow::Context;
use tracing::{instrument, warn};
use wascap::jwt;
use wasmtime_wasi::preview2;

wasmtime::component::bindgen!({
    world: "actor",
    async: true,
});

pub(super) struct Ctx {
    pub handler: Handler,
    pub wasi: preview2::WasiCtx,
    pub table: preview2::Table,
}

impl preview2::WasiView for Ctx {
    fn table(&self) -> &preview2::Table {
        &self.table
    }

    fn table_mut(&mut self) -> &mut preview2::Table {
        &mut self.table
    }

    fn ctx(&self) -> &preview2::WasiCtx {
        &self.wasi
    }

    fn ctx_mut(&mut self) -> &mut preview2::WasiCtx {
        &mut self.wasi
    }
}

impl Debug for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("handler", &self.handler)
            .field("runtime", &"wasmtime")
            .finish()
    }
}

/// Pre-compiled actor [Component], which is cheapily-[Cloneable](Clone)
#[derive(Clone)]
pub struct Component {
    component: wasmtime::component::Component,
    engine: wasmtime::Engine,
    claims: jwt::Claims<jwt::Actor>,
    handler: HandlerBuilder,
}

impl Debug for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Component")
            .field("claims", &self.claims)
            .field("handler", &self.handler)
            .field("runtime", &"wasmtime")
            .finish()
    }
}

impl Component {
    /// Extracts [Claims](jwt::Claims) from WebAssembly component and compiles it using [Runtime].
    #[instrument(skip(wasm))]
    pub fn new(rt: &Runtime, wasm: impl AsRef<[u8]>) -> anyhow::Result<Self> {
        let wasm = wasm.as_ref();
        let engine = rt.engine.clone();
        let claims = claims(wasm)?;
        let module = wasmtime::component::Component::new(&engine, wasm)
            .context("failed to compile component")?;
        Ok(Self {
            component: module,
            engine,
            claims,
            handler: rt.handler.clone(),
        })
    }

    /// [Claims](jwt::Claims) associated with this [Component].
    #[instrument]
    pub fn claims(&self) -> &jwt::Claims<jwt::Actor> {
        &self.claims
    }

    /// Returns an [ConfiguredComponent], which can be used to configure and produce an [Instance].
    #[instrument]
    pub fn configure(&self) -> ConfiguredComponent {
        self.into()
    }

    /// Like [Self::configure], but moves the [Component].
    #[instrument]
    pub fn into_configure(self) -> ConfiguredComponent {
        self.into()
    }

    /// Like [Self::configure], but moves the [Component] and returns the associated [jwt::Claims].
    #[instrument]
    pub fn into_configure_claims(self) -> (ConfiguredComponent, jwt::Claims<jwt::Actor>) {
        self.into()
    }

    /// Instantiates a [Component] and returns the resulting [Instance].
    #[instrument]
    pub async fn instantiate(&self) -> anyhow::Result<Instance> {
        self.configure().instantiate().await
    }

    /// Instantiates a [Component] producing an [Instance] and invokes an operation on it using [Instance::call]
    #[instrument(skip(operation, payload))]
    pub async fn call(
        &self,
        operation: impl AsRef<str>,
        payload: Option<impl AsRef<[u8]>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        self.configure().call(operation, payload).await
    }
}

/// A component paired with configuration
pub struct ConfiguredComponent {
    component: wasmtime::component::Component,
    engine: wasmtime::Engine,
    handler: HandlerBuilder,
    wasi: preview2::WasiCtxBuilder,
}

impl Debug for ConfiguredComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConfiguredComponent")
            .field("handler", &self.handler)
            .field("runtime", &"wasmtime")
            .finish()
    }
}

impl ConfiguredComponent {
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

    /// Configure component to inherit standard output of the process
    #[must_use]
    pub fn inherit_stdout(self) -> Self {
        Self {
            // TODO: simplify once https://github.com/bytecodealliance/wasmtime/pull/6465 lands
            wasi: self.wasi.set_stdout(preview2::stdio::stdout()),
            ..self
        }
    }

    /// Configure component to inherit standard error of the process
    #[must_use]
    pub fn inherit_stderr(self) -> Self {
        Self {
            // TODO: simplify once https://github.com/bytecodealliance/wasmtime/pull/6465 lands
            wasi: self.wasi.set_stderr(preview2::stdio::stderr()),
            ..self
        }
    }

    /// Instantiates a [ConfiguredComponent] and returns the resulting [Instance].
    #[instrument]
    pub async fn instantiate(self) -> anyhow::Result<Instance> {
        let mut linker = wasmtime::component::Linker::new(&self.engine);

        Interfaces::add_to_linker(&mut linker, |ctx: &mut Ctx| &mut ctx.handler)
            .context("failed to link `Wasmcloud` interface")?;

        preview2::wasi::command::add_to_linker(&mut linker)
            .context("failed to link `WASI` interface")?;

        let mut table = preview2::Table::new();
        let wasi = self
            .wasi
            .build(&mut table)
            .context("failed to build WASI")?;
        let ctx = Ctx {
            wasi,
            table,
            handler: self.handler.build(),
        };
        let store = wasmtime::Store::new(&self.engine, ctx);
        Ok(Instance {
            component: self.component,
            linker,
            store,
        })
    }

    /// Instantiates a [ConfiguredComponent] producing an [Instance] and invokes an operation on it using [Instance::call]
    #[instrument(skip(operation, payload))]
    pub async fn call(
        self,
        operation: impl AsRef<str>,
        payload: Option<impl AsRef<[u8]>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        let operation = operation.as_ref();
        self.instantiate()
            .await
            .context("failed to instantiate component")?
            .call(operation, payload)
            .await
            .with_context(|| format!("failed to call operation `{operation}` on component"))
    }
}

impl From<Component> for ConfiguredComponent {
    fn from(
        Component {
            component,
            engine,
            handler,
            ..
        }: Component,
    ) -> Self {
        Self {
            component,
            engine,
            handler,
            wasi: preview2::WasiCtxBuilder::new(),
        }
    }
}

impl From<Component> for (ConfiguredComponent, jwt::Claims<jwt::Actor>) {
    fn from(
        Component {
            component,
            engine,
            handler,
            claims,
        }: Component,
    ) -> Self {
        (
            ConfiguredComponent {
                component,
                engine,
                handler,
                wasi: preview2::WasiCtxBuilder::new(),
            },
            claims,
        )
    }
}

impl From<&Component> for ConfiguredComponent {
    fn from(
        Component {
            component,
            engine,
            handler,
            ..
        }: &Component,
    ) -> Self {
        Self {
            component: component.clone(),
            engine: engine.clone(),
            handler: handler.clone(),
            wasi: preview2::WasiCtxBuilder::new(),
        }
    }
}

/// An instance of a [Component]
pub struct Instance {
    component: wasmtime::component::Component,
    linker: wasmtime::component::Linker<Ctx>,
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
    /// Invoke an operation on an [Instance] producing a result, where outermost error represents
    /// a WebAssembly execution error and innermost - the component operation error
    #[instrument(skip_all)]
    pub async fn call(
        &mut self,
        operation: impl AsRef<str>,
        payload: Option<impl AsRef<[u8]>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        let (bindings, _) =
            Actor::instantiate_async(&mut self.store, &self.component, &self.linker)
                .await
                .context("failed to instantiate component `guest` interface")?;
        bindings
            .wasmcloud_bus_guest()
            .call_call(
                &mut self.store,
                operation.as_ref(),
                payload.as_ref().map(AsRef::as_ref),
            )
            .await
            .context("failed to call `guest.call`")
    }
}
