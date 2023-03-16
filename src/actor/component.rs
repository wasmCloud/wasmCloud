use super::actor_claims;

use crate::capability::{Handle, Invocation};
use crate::Runtime;

use core::fmt::{self, Debug};

use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use tracing::{instrument, warn};
use wascap::jwt;

use crate::actor::component::wasi_io::InputStream;
use crate::actor::component::wasi_io::OutputStream;
use crate::actor::component::wasi_io::StreamError;

use crate::actor::component::wasi_filesystem::Descriptor;
use crate::actor::component::wasi_filesystem::Filesize;
use crate::actor::component::wasi_filesystem::Errno;

wasmtime::component::bindgen!({
    world: "wasmcloud",
    async: true,
});

pub(super) struct Ctx<'a> {
    pub wasi: ::host::WasiCtx,
    pub claims: &'a jwt::Claims<jwt::Actor>,
    pub handler: Arc<Box<dyn Handle<Invocation>>>,
}

impl Debug for Ctx<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("runtime", &"wasmtime")
            .field("claims", &self.claims)
            .finish()
    }
}

impl<'a> Ctx<'a> {
    fn new(claims: &'a jwt::Claims<jwt::Actor>, handler: Arc<Box<dyn Handle<Invocation>>>) -> Self {
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
impl crate::actor::component::wasi_stderr::Host for Ctx<'_> {
    async fn print(&mut self, msg: String) -> anyhow::Result<()> {
        eprintln!("{msg}");
        Ok(())
    }
}

#[async_trait]
impl crate::actor::component::wasi_io::Host for Ctx<'_> {
    async fn drop_input_stream(&mut self, _is: InputStream) -> anyhow::Result<()> {
        // TODO: IMPLEMENT STUB
        Ok(())
    }

    async fn drop_output_stream(&mut self, _is: OutputStream) -> anyhow::Result<()> {
        // TODO: IMPLEMENT STUB
        Ok(())
    }

    async fn write(&mut self, _os: OutputStream, _buf: Vec<u8>) -> anyhow::Result<Result<u64, StreamError>> {
        // TODO: IMPLEMENT STUB
        anyhow::Result::Ok(Err(StreamError {}))
    }
}

#[async_trait]
impl crate::actor::component::wasi_filesystem::Host for Ctx<'_> {
    async fn write_via_stream(&mut self, _fd: Descriptor, _offset: Filesize) -> anyhow::Result<Result<OutputStream, Errno>> {
        // TODO: IMPLEMENT STUB
        anyhow::Result::Ok(Err(Errno::Busy))
    }

    async fn append_via_stream(&mut self, _fd: Descriptor) -> anyhow::Result<Result<OutputStream, Errno>> {
        // TODO: IMPLEMENT STUB
        anyhow::Result::Ok(Err(Errno::Busy))
    }

    async fn drop_descriptor(&mut self, _fd: Descriptor) -> anyhow::Result<()> {
        // TODO: IMPLEMENT STUB
        Ok(())
    }
}

#[async_trait]
impl crate::actor::component::wasi_exit::Host for Ctx<'_> {
    async fn exit(&mut self, _status: Result<(),()>) -> anyhow::Result<()> {
        // TODO: IMPLEMENT STUB
        Ok(())
    }
}

#[async_trait]
impl crate::actor::component::wasi_environment::Host for Ctx<'_> {
    async fn get_environment(&mut self) -> anyhow::Result<Vec<(String, String)>> {
        // TODO: IMPLEMENT STUB
        Ok(Vec::new())
    }
}

#[async_trait]
impl host::Host for Ctx<'_> {
    async fn host_call(
        &mut self,
        binding: String,
        namespace: String,
        operation: String,
        payload: Option<Vec<u8>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        let invocation = (namespace, operation, payload)
            .try_into()
            .context("failed to parse invocation")?;
        match self.handler.handle(self.claims, binding, invocation).await {
            Err(err) => Ok(Err(err.to_string())),
            Ok(res) => Ok(Ok(res)),
        }
    }
}

/// Pre-compiled actor [Component], which is cheapily-[Cloneable](Clone)
#[derive(Clone)]
pub struct Component {
    component: wasmtime::component::Component,
    engine: wasmtime::Engine,
    claims: jwt::Claims<jwt::Actor>,
    handler: Arc<Box<dyn Handle<Invocation>>>,
}

impl Debug for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Component")
            .field("runtime", &"wasmtime")
            .field("claims", &self.claims)
            .finish()
    }
}

impl Component {
    /// [Claims](jwt::Claims) associated with this [Component].
    #[instrument]
    pub fn claims(&self) -> &jwt::Claims<jwt::Actor> {
        &self.claims
    }
}

impl Component {
    /// Extracts [Claims](jwt::Claims) from WebAssembly component and compiles it using [Runtime].
    #[instrument(skip(wasm))]
    pub fn new(rt: &Runtime, wasm: impl AsRef<[u8]>) -> anyhow::Result<Self> {
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

    /// Instantiates a [Component] and returns the resulting [Instance].
    #[instrument(skip_all)]
    pub async fn instantiate(&self) -> anyhow::Result<Instance> {
        let mut linker = wasmtime::component::Linker::new(&self.engine);

        Wasmcloud::add_to_linker(&mut linker, |ctx: &mut Ctx<'_>| ctx)
            .context("failed to link `Wasmcloud` interface")?;

        ::host::add_to_linker(&mut linker, |ctx: &mut Ctx<'_>| &mut ctx.wasi)
            .context("failed to link `WASI` interface")?;

        let cx = Ctx::new(&self.claims, Arc::clone(&self.handler));
        let mut store = wasmtime::Store::new(&self.engine, cx);

        let (bindings, _) = Wasmcloud::instantiate_async(&mut store, &self.component, &linker)
            .await
            .context("failed to instantiate component")?;

        Ok(Instance { bindings, store })
    }

    /// Instantiate a [Component] producing an [Instance] and invoke an operation on it using [Instance::call]
    #[instrument(skip(operation, payload))]
    pub async fn call(
        &self,
        operation: impl AsRef<str>,
        payload: Option<impl AsRef<[u8]>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        let operation = operation.as_ref();
        let mut instance = self
            .instantiate()
            .await
            .context("failed to instantiate component")?;
        instance
            .call(operation, payload)
            .await
            .context("failed to call operation `{operation}` on module")
    }
}

/// An instance of a [Component]
pub struct Instance<'a> {
    bindings: Wasmcloud,
    store: wasmtime::Store<Ctx<'a>>,
}

impl Instance<'_> {
    /// Invoke an operation on an [Instance] producing a result, where outermost error represents
    /// a WebAssembly execution error and innermost - the component operation error
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
