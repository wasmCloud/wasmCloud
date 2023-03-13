use super::actor_claims;

use crate::{capability, Runtime};

use core::fmt::{self, Debug};

use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use futures::AsyncReadExt;
use tracing::{instrument, warn};
use wascap::jwt;

wasmtime::component::bindgen!({
    world: "wasmcloud",
    async: true,
});

pub(super) struct Ctx<'a, H> {
    pub wasi: ::host::WasiCtx,
    pub claims: &'a jwt::Claims<jwt::Actor>,
    pub handler: Arc<H>,
}

impl<H> Debug for Ctx<'_, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("runtime", &"wasmtime")
            .field("claims", &self.claims)
            .finish()
    }
}

impl<'a, H> Ctx<'a, H> {
    fn new(claims: &'a jwt::Claims<jwt::Actor>, handler: Arc<H>) -> Self {
        // TODO: Set stdio pipes
        let wasi = wasi_cap_std_sync::WasiCtxBuilder::new().build();
        Self {
            wasi,
            claims,
            handler,
        }
    }
}

#[async_trait]
impl<H: capability::Handler> host::Host for Ctx<'_, H> {
    async fn host_call(
        &mut self,
        binding: String,
        namespace: String,
        operation: String,
        payload: Option<Vec<u8>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        match self
            .handler
            .handle(self.claims, binding, namespace, operation, payload)
            .await
        {
            Err(err) => Err(err),
            Ok(Err(err)) => Ok(Err(err.to_string())),
            Ok(Ok(res)) => Ok(Ok(res)),
        }
    }
}

/// Pre-compiled actor [Component], which is cheapily-[Cloneable](Clone)
#[derive(Clone)]
pub struct Component<H> {
    component: wasmtime::component::Component,
    engine: wasmtime::Engine,
    claims: jwt::Claims<jwt::Actor>,
    handler: Arc<H>,
}

impl<H> Debug for Component<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Component")
            .field("runtime", &"wasmtime")
            .field("claims", &self.claims)
            .finish()
    }
}

impl<H> Component<H> {
    /// [Claims](jwt::Claims) associated with this [Component].
    #[instrument]
    pub fn claims(&self) -> &jwt::Claims<jwt::Actor> {
        &self.claims
    }
}

impl<H: capability::Handler + 'static> Component<H> {
    /// Extracts [Claims](jwt::Claims) from WebAssembly component and compiles it using [Runtime].
    #[instrument(skip(wasm))]
    pub fn new(rt: &Runtime<H>, wasm: impl AsRef<[u8]>) -> anyhow::Result<Self> {
        let wasm = wasm.as_ref();
        let engine = rt.engine.clone();
        let claims = actor_claims(wasm)?;
        let module = wasmtime::component::Component::new(&engine, wasm)
            .context("failed to compile component")?;
        Ok(Self {
            component: module,
            engine,
            claims,
            handler: Arc::clone(&rt.handler),
        })
    }

    /// Reads the WebAssembly module asynchronously and calls [Component::new].
    #[instrument(skip(wasm))]
    pub async fn read(
        rt: &Runtime<H>,
        mut wasm: impl futures::AsyncRead + Unpin,
    ) -> anyhow::Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf)
            .await
            .context("failed to read Wasm")?;
        Self::new(rt, buf)
    }

    /// Reads the WebAssembly module synchronously and calls [Component::new].
    #[instrument(skip(wasm))]
    pub fn read_sync(rt: &Runtime<H>, mut wasm: impl std::io::Read) -> anyhow::Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf).context("failed to read Wasm")?;
        Self::new(rt, buf)
    }

    /// Instantiates a [Component] and returns the resulting [Instance].
    #[instrument(skip_all)]
    pub async fn instantiate(&self) -> anyhow::Result<Instance<H>>
    where
        H: capability::Handler + Sync + Send + 'static,
    {
        let mut linker = wasmtime::component::Linker::new(&self.engine);

        Wasmcloud::add_to_linker(&mut linker, |ctx: &mut Ctx<'_, H>| ctx)
            .context("failed to link `Wasmcloud` interface")?;

        ::host::add_to_linker(&mut linker, |ctx: &mut Ctx<'_, H>| &mut ctx.wasi)
            .context("failed to link `WASI` interface")?;

        let cx = Ctx::new(&self.claims, Arc::clone(&self.handler));
        let mut store = wasmtime::Store::new(&self.engine, cx);

        let (bindings, _) = Wasmcloud::instantiate_async(&mut store, &self.component, &linker)
            .await
            .context("failed to instantiate component")?;
        Ok(Instance { bindings, store })
    }
}

/// An instance of a [Module]
pub struct Instance<'a, H> {
    bindings: Wasmcloud,
    store: wasmtime::Store<Ctx<'a, H>>,
}

impl<H: Sync + Send> Instance<'_, H> {
    /// Invoke an operation on an [Instance] producing a [Response].
    #[instrument(skip_all)]
    pub async fn call(
        &mut self,
        operation: impl AsRef<str>,
        payload: Option<impl AsRef<[u8]>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        self.bindings
            .actor()
            .call_guest_call(
                &mut self.store,
                operation.as_ref(),
                payload.as_ref().map(AsRef::as_ref),
            )
            .await
            .context("failed to call `guest-call`")
    }
}
