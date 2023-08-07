use crate::actor::claims;
use crate::capability::{builtin, Interfaces};
use crate::Runtime;

use core::fmt::{self, Debug};
use core::mem::replace;
use core::ops::{Deref, DerefMut};

use std::sync::Arc;

use anyhow::Context as _;
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use tracing::{instrument, trace};
use wascap::jwt;
use wasmtime_wasi::preview2::wasi::command::Command;
use wasmtime_wasi::preview2::{self, InputStream, OutputStream};

mod blobstore;
mod bus;
mod http;
mod logging;
mod messaging;

pub(crate) use self::http::incoming_http_bindings;
pub(crate) use self::logging::logging_bindings;

mod guest_bindings {
    wasmtime::component::bindgen!({
        world: "guest",
        async: true,
        with: {
           "wasi:io/streams": wasmtime_wasi::preview2::wasi::io::streams,
           "wasi:poll/poll": wasmtime_wasi::preview2::wasi::poll::poll,
        },
    });
}

struct AsyncStream<T>(T);

#[async_trait]
impl<T: AsyncRead + Send + Sync + Unpin + 'static> InputStream for AsyncStream<T> {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    #[instrument(skip(self))]
    async fn read(&mut self, buf: &mut [u8]) -> anyhow::Result<(u64, bool)> {
        let n = self.0.read(buf).await.context("failed to read")?;
        let n = n.try_into().context("overflow")?;
        Ok((n, !buf.is_empty() && n == 0))
    }

    async fn read_vectored<'a>(
        &mut self,
        bufs: &mut [std::io::IoSliceMut<'a>],
    ) -> anyhow::Result<(u64, bool)> {
        for buf in bufs {
            if buf.len() > 0 {
                let n = self.0.read(buf).await.context("failed to read")?;
                let n = n.try_into().context("overflow")?;
                return Ok((n, n == 0));
            }
        }
        Ok((0, false))
    }

    fn is_read_vectored(&self) -> bool {
        true
    }

    async fn readable(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl<T: AsyncWrite + Send + Sync + Unpin + 'static> OutputStream for AsyncStream<T> {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    #[instrument(skip(self))]
    async fn write(&mut self, buf: &[u8]) -> anyhow::Result<u64> {
        let n = self.0.write(buf).await.context("failed to write")?;
        let n = n.try_into().context("overflow")?;
        Ok(n)
    }

    #[instrument(skip(self))]
    async fn write_vectored<'a>(&mut self, bufs: &[std::io::IoSlice<'a>]) -> anyhow::Result<u64> {
        let n = self
            .0
            .write_vectored(bufs)
            .await
            .context("failed to write")?;
        let n = n.try_into().context("overflow")?;
        Ok(n)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    async fn writable(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// `StdioStream` delegates all stream I/O to inner [`AsyncStream`] if such is set and
/// mimics [`std::io::empty`] and [`std::io::sink`] otherwise
struct StdioStream<T>(Arc<Mutex<Option<AsyncStream<T>>>>);

impl<T> Clone for StdioStream<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Default for StdioStream<T> {
    fn default() -> Self {
        Self(Arc::default())
    }
}

impl<T> Deref for StdioStream<T> {
    type Target = Arc<Mutex<Option<AsyncStream<T>>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for StdioStream<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T> StdioStream<T> {
    /// Replace the inner stream by another one returning the previous one if such was set
    async fn replace(&self, stream: T) -> Option<AsyncStream<T>> {
        self.lock().await.replace(AsyncStream(stream))
    }

    /// Replace the inner stream by another one returning the previous one if such was set
    async fn take(&self) -> Option<AsyncStream<T>> {
        self.lock().await.take()
    }
}

#[async_trait]
impl<T: AsyncRead + Send + Sync + Unpin + 'static> InputStream for StdioStream<T> {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    #[instrument(skip(self))]
    async fn read(&mut self, buf: &mut [u8]) -> anyhow::Result<(u64, bool)> {
        if let Some(stream) = self.0.lock().await.as_mut() {
            stream.read(buf).await
        } else {
            Ok((0, true))
        }
    }

    #[instrument(skip(self))]
    async fn read_vectored<'a>(
        &mut self,
        bufs: &mut [std::io::IoSliceMut<'a>],
    ) -> anyhow::Result<(u64, bool)> {
        if let Some(stream) = self.0.lock().await.as_mut() {
            stream.read_vectored(bufs).await
        } else {
            Ok((0, true))
        }
    }

    fn is_read_vectored(&self) -> bool {
        true
    }

    async fn readable(&self) -> anyhow::Result<()> {
        if let Some(stream) = self.0.lock().await.as_ref() {
            stream.readable().await
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl<T: AsyncWrite + Send + Sync + Unpin + 'static> OutputStream for StdioStream<T> {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    #[instrument(skip(self))]
    async fn write(&mut self, buf: &[u8]) -> anyhow::Result<u64> {
        if let Some(stream) = self.0.lock().await.as_mut() {
            stream.write(buf).await
        } else {
            Ok(buf.len().try_into().unwrap_or(u64::MAX))
        }
    }

    #[instrument(skip(self))]
    async fn write_vectored<'a>(&mut self, bufs: &[std::io::IoSlice<'a>]) -> anyhow::Result<u64> {
        if let Some(stream) = self.0.lock().await.as_mut() {
            stream.write_vectored(bufs).await
        } else {
            let total = bufs.iter().map(|b| b.len()).sum::<usize>();
            Ok(total.try_into().unwrap_or(u64::MAX))
        }
    }

    // TODO: Implement `splice`
    //async fn splice(
    //    &mut self,
    //    src: &mut dyn InputStream,
    //    nelem: u64,
    //) -> anyhow::Result<(u64, bool)> {
    //    todo!()
    //}

    #[instrument(skip(self))]
    async fn write_zeroes(&mut self, nelem: u64) -> anyhow::Result<u64> {
        if let Some(stream) = self.0.lock().await.as_mut() {
            stream.write_zeroes(nelem).await
        } else {
            Ok(nelem)
        }
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    async fn writable(&self) -> anyhow::Result<()> {
        if let Some(stream) = self.0.lock().await.as_ref() {
            stream.writable().await
        } else {
            Ok(())
        }
    }
}

struct Ctx {
    wasi: preview2::WasiCtx,
    table: preview2::Table,
    handler: builtin::Handler,
    stdin: StdioStream<Box<dyn AsyncRead + Send + Sync + Unpin>>,
    stdout: StdioStream<Box<dyn AsyncWrite + Send + Sync + Unpin>>,
    stderr: StdioStream<Box<dyn AsyncWrite + Send + Sync + Unpin>>,
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
        f.debug_struct("Ctx").field("runtime", &"wasmtime").finish()
    }
}

/// Pre-compiled actor [Component], which is cheapily-[Cloneable](Clone)
#[derive(Clone)]
pub struct Component {
    component: wasmtime::component::Component,
    engine: wasmtime::Engine,
    claims: Option<jwt::Claims<jwt::Actor>>,
    handler: builtin::HandlerBuilder,
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

fn instantiate(
    engine: &wasmtime::Engine,
    component: wasmtime::component::Component,
    handler: impl Into<builtin::Handler>,
) -> anyhow::Result<Instance> {
    let mut linker = wasmtime::component::Linker::new(engine);

    Interfaces::add_to_linker(&mut linker, |ctx| ctx)
        .context("failed to link `Wasmcloud` interface")?;

    preview2::wasi::command::add_to_linker(&mut linker)
        .context("failed to link `WASI` interface")?;

    let stdin = StdioStream::default();
    let stdout = StdioStream::default();
    let stderr = StdioStream::default();

    // NOTE: stdio will be added to table by `build()` below
    let mut table = preview2::Table::new();
    let wasi = preview2::WasiCtxBuilder::new()
        .set_args(&["main.wasm"]) // TODO: Configure argv[0]
        .set_stdin(stdin.clone())
        .set_stdout(stdout.clone())
        .set_stderr(stderr.clone())
        .build(&mut table)
        .context("failed to build WASI")?;
    let handler = handler.into();
    let ctx = Ctx {
        wasi,
        table,
        handler,
        stdin,
        stdout,
        stderr,
    };
    let store = wasmtime::Store::new(engine, ctx);
    Ok(Instance {
        component,
        linker,
        store,
    })
}

impl Component {
    /// Extracts [Claims](jwt::Claims) from WebAssembly component and compiles it using [Runtime].
    #[instrument(skip(wasm))]
    pub fn new(rt: &Runtime, wasm: impl AsRef<[u8]>) -> anyhow::Result<Self> {
        let wasm = wasm.as_ref();
        let engine = rt.engine.clone();
        let claims = claims(wasm)?;
        let component = wasmtime::component::Component::new(&engine, wasm)
            .context("failed to compile component")?;
        Ok(Self {
            component,
            engine,
            claims,
            handler: rt.handler.clone(),
        })
    }

    /// [Claims](jwt::Claims) associated with this [Component].
    #[instrument]
    pub fn claims(&self) -> Option<&jwt::Claims<jwt::Actor>> {
        self.claims.as_ref()
    }

    /// Like [Self::instantiate], but moves the [Component].
    #[instrument]
    pub fn into_instance(self) -> anyhow::Result<Instance> {
        self.instantiate()
    }

    /// Like [Self::instantiate], but moves the [Component] and returns the associated [jwt::Claims].
    #[instrument]
    pub fn into_instance_claims(
        self,
    ) -> anyhow::Result<(Instance, Option<jwt::Claims<jwt::Actor>>)> {
        let instance = instantiate(&self.engine, self.component, self.handler)?;
        Ok((instance, self.claims))
    }

    /// Instantiates a [Component] and returns the resulting [Instance].
    #[instrument]
    pub fn instantiate(&self) -> anyhow::Result<Instance> {
        instantiate(&self.engine, self.component.clone(), self.handler.clone())
    }

    /// Instantiates a [Component] producing an [Instance] and invokes an operation on it using [Instance::call]
    #[instrument(skip_all)]
    pub async fn call(
        &self,
        operation: impl AsRef<str>,
        request: impl AsyncRead + Send + Sync + Unpin + 'static,
        response: impl AsyncWrite + Send + Sync + Unpin + 'static,
    ) -> anyhow::Result<Result<(), String>> {
        self.instantiate()
            .context("failed to instantiate component")?
            .call(operation, request, response)
            .await
    }
}

impl From<Component> for Option<jwt::Claims<jwt::Actor>> {
    fn from(Component { claims, .. }: Component) -> Self {
        claims
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
    /// Returns a mutable reference to embedded [`builtin::Handler`]
    fn handler_mut(&mut self) -> &mut builtin::Handler {
        &mut self.store.data_mut().handler
    }

    /// Reset [`Instance`] state to defaults
    pub async fn reset(&mut self, rt: &Runtime) {
        *self.handler_mut() = rt.handler.clone().into();
        let ctx = self.store.data_mut();
        ctx.stderr.take().await;
    }

    /// Set actor stderr stream. If another stderr was set, it is replaced and the old one is flushed and shut down.
    ///
    /// # Errors
    ///
    /// Fails if flushing and shutting down old stream fails
    pub async fn stderr(
        &mut self,
        stderr: impl AsyncWrite + Send + Sync + Unpin + 'static,
    ) -> anyhow::Result<&mut Self> {
        let ctx = self.store.data();
        if let Some(AsyncStream(mut stderr)) = ctx.stderr.replace(Box::new(stderr)).await {
            stderr.flush().await.context("failed to flush stderr")?;
            stderr
                .shutdown()
                .await
                .context("failed to shutdown stderr")?;
            Ok(self)
        } else {
            Ok(self)
        }
    }

    /// Instantiates and returns [`GuestBindings`] if exported by the [`Instance`].
    async fn as_guest_bindings(&mut self) -> anyhow::Result<GuestBindings> {
        if let Ok((bindings, _)) =
            guest_bindings::Guest::instantiate_async(&mut self.store, &self.component, &self.linker)
                .await
        {
            Ok(GuestBindings::Interface(bindings))
        } else {
            let (bindings, _) = Command::instantiate_async(&mut self.store, &self.component, &self.linker).await.context(
                    "failed to instantiate either `wasmcloud::bus/guest` interface or get `wasi:command/command`",
                )?;
            Ok(GuestBindings::Command(bindings))
        }
    }

    /// Invoke an operation on an [Instance] producing a result.
    #[instrument(skip_all)]
    pub async fn call(
        &mut self,
        operation: impl AsRef<str>,
        request: impl AsyncRead + Send + Sync + Unpin + 'static,
        response: impl AsyncWrite + Send + Sync + Unpin + 'static,
    ) -> anyhow::Result<Result<(), String>> {
        self.as_guest_bindings()
            .await?
            .call(&mut self.store, operation, request, response)
            .await
    }

    /// Instantiates and returns a [`GuestInstance`] if exported by the [`Instance`].
    ///
    /// # Errors
    ///
    /// Fails if guest bindings are not exported by the [`Instance`]
    pub async fn into_guest(mut self) -> anyhow::Result<GuestInstance> {
        let bindings = self.as_guest_bindings().await?;
        Ok(GuestInstance {
            store: Arc::new(Mutex::new(self.store)),
            bindings: Arc::new(bindings),
        })
    }
}

enum GuestBindings {
    Command(Command),
    Interface(guest_bindings::Guest),
}

impl GuestBindings {
    /// Invoke an operation on a [GuestBindings] producing a result.
    #[instrument(skip_all)]
    pub async fn call(
        &self,
        mut store: &mut wasmtime::Store<Ctx>,
        operation: impl AsRef<str>,
        request: impl AsyncRead + Send + Sync + Unpin + 'static,
        response: impl AsyncWrite + Send + Sync + Unpin + 'static,
    ) -> anyhow::Result<Result<(), String>> {
        let ctx = store.data_mut();
        ctx.stdin.replace(Box::new(request)).await;
        ctx.stdout.replace(Box::new(response)).await;
        let res = match self {
            GuestBindings::Command(bindings) => {
                let wasi = preview2::WasiCtxBuilder::new()
                    .set_args(&["main.wasm", operation.as_ref()]) // TODO: Configure argv[0]
                    .set_stdin(ctx.stdin.clone())
                    .set_stdout(ctx.stdout.clone())
                    .set_stderr(ctx.stderr.clone())
                    .build(&mut ctx.table)
                    .context("failed to build WASI")?;
                let wasi = replace(&mut ctx.wasi, wasi);
                trace!("call `wasi:command/command.run`");
                let res = bindings
                    .call_run(&mut store)
                    .await
                    .context("failed to call `wasi:command/command.run`")?
                    .map_err(|()| "`wasi:command/command.run` failed".to_string());
                store.data_mut().wasi = wasi;
                Ok(res)
            }
            GuestBindings::Interface(bindings) => {
                trace!("call `wasmcloud:bus/guest.call`");
                bindings
                    .wasmcloud_bus_guest()
                    .call_call(&mut store, operation.as_ref())
                    .await
                    .context("failed to call `wasmcloud:bus/guest.call`")
            }
        };
        let ctx = store.data();
        ctx.stdin.take().await.context("stdin missing")?;
        let AsyncStream(mut stdout) = ctx.stdout.take().await.context("stdout missing")?;
        trace!("flush stdout");
        stdout.flush().await.context("failed to flush stdout")?;
        trace!("shutdown stdout");
        stdout
            .shutdown()
            .await
            .context("failed to shutdown stdout")?;
        res
    }
}

/// Instantiated, clone-able guest instance
#[derive(Clone)]
pub struct GuestInstance {
    store: Arc<Mutex<wasmtime::Store<Ctx>>>,
    bindings: Arc<GuestBindings>,
}

impl GuestInstance {
    /// Invoke an operation on a [GuestInstance] producing a result.
    #[instrument(skip_all)]
    pub async fn call(
        &self,
        operation: impl AsRef<str>,
        request: impl AsyncRead + Send + Sync + Unpin + 'static,
        response: impl AsyncWrite + Send + Sync + Unpin + 'static,
    ) -> anyhow::Result<Result<(), String>> {
        let mut store = self.store.lock().await;
        self.bindings
            .call(&mut store, operation, request, response)
            .await
    }
}

enum InterfaceBindings<T> {
    Guest(GuestBindings),
    Interface(T),
}

/// Instance of a guest interface `T`
pub struct InterfaceInstance<T> {
    store: Mutex<wasmtime::Store<Ctx>>,
    bindings: InterfaceBindings<T>,
}
