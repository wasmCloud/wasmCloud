use crate::actor::claims;
use crate::capability::{builtin, Interfaces};
use crate::Runtime;

use core::fmt::{self, Debug};
use core::mem::replace;
use core::ops::{Deref, DerefMut};

use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use tracing::{instrument, trace};
use wascap::jwt;
use wasmtime_wasi::preview2::command::Command;
use wasmtime_wasi::preview2::pipe::{
    AsyncReadStream, AsyncWriteStream, ClosedInputStream, ClosedOutputStream,
};
use wasmtime_wasi::preview2::{
    self, HostInputStream, HostOutputStream, OutputStreamError, StreamState,
};

mod blobstore;
mod bus;
mod http;
mod keyvalue;
mod logging;
mod messaging;

pub(crate) use self::http::incoming_http_bindings;
pub(crate) use self::logging::logging_bindings;

type TableResult<T> = Result<T, preview2::TableError>;

mod guest_bindings {
    wasmtime::component::bindgen!({
        world: "guest",
        async: true,
        with: {
           "wasi:io/streams": wasmtime_wasi::preview2::bindings::io::streams,
           "wasi:poll/poll": wasmtime_wasi::preview2::bindings::poll::poll,
        },
    });
}

/// `StdioStream` delegates all stream I/O to inner stream if such is set and
/// mimics [`ClosedInputStream`] and [`ClosedOutputStream`] otherwise
struct StdioStream<T>(Arc<Mutex<Option<T>>>);

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
    type Target = Arc<Mutex<Option<T>>>;

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
    async fn replace(&self, stream: T) -> Option<T> {
        self.0.lock().await.replace(stream)
    }

    /// Replace the inner stream by another one returning the previous one if such was set
    async fn take(&self) -> Option<T> {
        self.0.lock().await.take()
    }
}

#[async_trait]
impl HostInputStream for StdioStream<Box<dyn HostInputStream>> {
    #[instrument(level = "trace", skip(self))]
    fn read(&mut self, size: usize) -> anyhow::Result<(Bytes, StreamState)> {
        match self.0.try_lock().as_deref_mut() {
            Ok(None) => ClosedInputStream.read(size),
            Ok(Some(stream)) => stream.read(size),
            Err(_) => Ok((Bytes::default(), StreamState::Open)),
        }
    }

    #[instrument(level = "trace", skip(self))]
    fn skip(&mut self, nelem: usize) -> anyhow::Result<(usize, StreamState)> {
        match self.0.try_lock().as_deref_mut() {
            Ok(None) => ClosedInputStream.skip(nelem),
            Ok(Some(stream)) => stream.skip(nelem),
            Err(_) => Ok((0, StreamState::Open)),
        }
    }

    #[instrument(level = "trace", skip(self))]
    async fn ready(&mut self) -> anyhow::Result<()> {
        if let Some(stream) = self.0.lock().await.as_mut() {
            stream.ready().await
        } else {
            ClosedInputStream.ready().await
        }
    }
}

type OutputStreamResult<T> = Result<T, OutputStreamError>;

#[async_trait]
impl HostOutputStream for StdioStream<Box<dyn HostOutputStream>> {
    #[instrument(level = "trace", skip(self))]
    fn write(&mut self, bytes: Bytes) -> OutputStreamResult<()> {
        match self.0.try_lock().as_deref_mut() {
            Ok(None) => ClosedOutputStream.write(bytes),
            Ok(Some(stream)) => stream.write(bytes),
            Err(_) => Err(OutputStreamError::Trap(anyhow!("deadlock"))),
        }
    }

    #[instrument(level = "trace", skip(self))]
    fn write_zeroes(&mut self, nelem: usize) -> OutputStreamResult<()> {
        match self.0.try_lock().as_deref_mut() {
            Ok(None) => ClosedOutputStream.write_zeroes(nelem),
            Ok(Some(stream)) => stream.write_zeroes(nelem),
            Err(_) => Err(OutputStreamError::Trap(anyhow!("deadlock"))),
        }
    }

    #[instrument(level = "trace", skip(self))]
    fn flush(&mut self) -> OutputStreamResult<()> {
        match self.0.try_lock().as_deref_mut() {
            Ok(None) => ClosedOutputStream.flush(),
            Ok(Some(stream)) => stream.flush(),
            Err(_) => Err(OutputStreamError::Trap(anyhow!("deadlock"))),
        }
    }

    #[instrument(level = "trace", skip(self))]
    async fn write_ready(&mut self) -> OutputStreamResult<usize> {
        if let Some(stream) = self.0.lock().await.as_mut() {
            stream.write_ready().await
        } else {
            ClosedOutputStream.write_ready().await
        }
    }
}

struct Ctx {
    wasi: preview2::WasiCtx,
    table: preview2::Table,
    handler: builtin::Handler,
    stdin: StdioStream<Box<dyn HostInputStream>>,
    stdout: StdioStream<Box<dyn HostOutputStream>>,
    stderr: StdioStream<Box<dyn HostOutputStream>>,
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
            .finish_non_exhaustive()
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

    preview2::command::add_to_linker(&mut linker).context("failed to link `WASI` interface")?;

    let stdin = StdioStream::default();
    let stdout = StdioStream::default();
    let stderr = StdioStream::default();

    // NOTE: stdio will be added to table by `build()` below
    let mut table = preview2::Table::new();
    let wasi = preview2::WasiCtxBuilder::new()
        .args(&["main.wasm"]) // TODO: Configure argv[0]
        .stdin(stdin.clone(), preview2::IsATTY::No)
        .stdout(stdout.clone(), preview2::IsATTY::No)
        .stderr(stderr.clone(), preview2::IsATTY::No)
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
    #[instrument(level = "trace")]
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
    #[instrument(level = "trace", skip_all)]
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
    /// Fails if flushing old stream fails
    pub async fn stderr(
        &mut self,
        stderr: impl AsyncWrite + Send + Sync + Unpin + 'static,
    ) -> anyhow::Result<&mut Self> {
        let data = self.store.data();
        if let Some(mut stderr) = data
            .stderr
            .replace(Box::new(AsyncWriteStream::new(1 << 16, stderr)))
            .await
        {
            stderr.flush().context("failed to flush stderr")?;
        }
        Ok(self)
    }

    /// Instantiates and returns [`GuestBindings`] if exported by the [`Instance`].
    async fn as_guest_bindings(&mut self) -> anyhow::Result<GuestBindings> {
        // Attempt to instantiate using guest bindings
        let guest_err = match guest_bindings::Guest::instantiate_async(
            &mut self.store,
            &self.component,
            &self.linker,
        )
        .await
        {
            Ok((bindings, _)) => return Ok(GuestBindings::Interface(bindings)),
            Err(e) => e,
        };

        // Attempt to instantiate using only bindings available in command
        match Command::instantiate_async(&mut self.store, &self.component, &self.linker).await {
            Ok((bindings, _)) => Ok(GuestBindings::Command(bindings)),
            // If neither of the above instantiations worked, the instance cannot be run
            Err(command_err) => bail!(
                r#"failed to instantiate instance (no bindings satisfied exports):

`wasmcloud:bus/guest` error: {guest_err:?}

`wasi:command/command` error: {command_err:?}
"#,
            ),
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
        ctx.stdin
            .replace(Box::new(AsyncReadStream::new(request)))
            .await;
        ctx.stdout
            .replace(Box::new(AsyncWriteStream::new(1 << 16, response)))
            .await;
        let res = match self {
            GuestBindings::Command(bindings) => {
                let operation = operation.as_ref();
                let wasi = preview2::WasiCtxBuilder::new()
                    .args(&["main.wasm", operation]) // TODO: Configure argv[0]
                    .stdin(ctx.stdin.clone(), preview2::IsATTY::No)
                    .stdout(ctx.stdout.clone(), preview2::IsATTY::No)
                    .stderr(ctx.stderr.clone(), preview2::IsATTY::No)
                    .build(&mut ctx.table)
                    .context("failed to build WASI")?;
                let wasi = replace(&mut ctx.wasi, wasi);
                trace!(operation, "call `wasi:command/command.run`");
                let res = bindings
                    .wasi_cli_run()
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
        let mut stdout = ctx.stdout.take().await.context("stdout missing")?;
        trace!("flush stdout");
        stdout.flush().context("failed to flush stdout")?;
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
    #[instrument(level = "trace", skip_all)]
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
