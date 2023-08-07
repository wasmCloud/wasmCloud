use crate::actor::claims;
use crate::capability::bus::host;
use crate::capability::logging::logging;
use crate::capability::{
    blobstore, builtin, messaging, Bus, IncomingHttp, Interfaces, Logging, Messaging,
};
use crate::Runtime;

use core::fmt::{self, Debug};
use core::future::Future;
use core::mem::replace;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use futures::future::Shared;
use futures::lock::Mutex;
use futures::FutureExt;
use serde_json::json;
use tokio::io::{sink, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tracing::{instrument, trace, warn};
use wascap::jwt;
use wasmtime_wasi::preview2::stream::TableStreamExt;
use wasmtime_wasi::preview2::wasi::command::Command;
use wasmtime_wasi::preview2::{self, InputStream, OutputStream};

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

pub(crate) mod logging_bindings {
    wasmtime::component::bindgen!({
        world: "logging",
        async: true,
        with: {
           "wasi:logging/logging": crate::capability::logging,
        },
    });
}

pub(crate) mod incoming_http_bindings {
    wasmtime::component::bindgen!({
        world: "incoming-http",
        async: true,
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

type FutureResult = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

pub trait TableFutureResultExt {
    fn push_future_result(&mut self, res: FutureResult) -> Result<u32, preview2::TableError>;
    fn get_future_result(
        &mut self,
        res: u32,
    ) -> Result<Box<Shared<FutureResult>>, preview2::TableError>;
    fn delete_future_result(
        &mut self,
        res: u32,
    ) -> Result<Box<Shared<FutureResult>>, preview2::TableError>;
}
impl TableFutureResultExt for preview2::Table {
    fn push_future_result(&mut self, res: FutureResult) -> Result<u32, preview2::TableError> {
        self.push(Box::new(res.shared()))
    }
    fn get_future_result(
        &mut self,
        res: u32,
    ) -> Result<Box<Shared<FutureResult>>, preview2::TableError> {
        self.get(res).cloned()
    }
    fn delete_future_result(
        &mut self,
        res: u32,
    ) -> Result<Box<Shared<FutureResult>>, preview2::TableError> {
        self.delete(res)
    }
}

#[async_trait]
impl host::Host for Ctx {
    #[instrument]
    async fn call(
        &mut self,
        operation: String,
    ) -> anyhow::Result<
        Result<
            (
                host::FutureResult,
                preview2::wasi::io::streams::InputStream,
                preview2::wasi::io::streams::OutputStream,
            ),
            String,
        >,
    > {
        match self.handler.call(operation).await {
            Ok((result, stdin, stdout)) => {
                let result = self
                    .table
                    .push_future_result(result)
                    .context("failed to push result to table")?;
                let stdin = self
                    .table
                    .push_output_stream(Box::new(AsyncStream(stdin)))
                    .context("failed to push stdin stream")?;
                let stdout = self
                    .table
                    .push_input_stream(Box::new(AsyncStream(stdout)))
                    .context("failed to push stdout stream")?;
                Ok(Ok((result, stdin, stdout)))
            }
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn listen_to_future_result(&mut self, _res: u32) -> anyhow::Result<u32> {
        bail!("unsupported") // TODO: Support
    }

    #[instrument]
    async fn future_result_get(&mut self, res: u32) -> anyhow::Result<Option<Result<(), String>>> {
        let fut = self.table.get_future_result(res)?;
        if let Some(result) = fut.clone().now_or_never() {
            let fut = self.table.delete_future_result(res)?;
            drop(fut);
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }

    #[instrument]
    async fn drop_future_result(&mut self, res: u32) -> anyhow::Result<()> {
        let fut = self.table.delete_future_result(res)?;
        drop(fut);
        Ok(())
    }
}

#[async_trait]
impl logging::Host for Ctx {
    #[instrument]
    async fn log(
        &mut self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        self.handler.log(level, context, message).await
    }
}

#[async_trait]
impl messaging::types::Host for Ctx {}

#[async_trait]
impl messaging::consumer::Host for Ctx {
    #[instrument]
    async fn request(
        &mut self,
        subject: String,
        body: Option<Vec<u8>>,
        timeout_ms: u32,
    ) -> anyhow::Result<Result<messaging::types::BrokerMessage, String>> {
        let timeout = Duration::from_millis(timeout_ms.into());
        Ok(self
            .handler
            .request(subject, body, timeout)
            .await
            .map_err(|err| format!("{err:#}")))
    }

    #[instrument]
    async fn request_multi(
        &mut self,
        subject: String,
        body: Option<Vec<u8>>,
        timeout_ms: u32,
        max_results: u32,
    ) -> anyhow::Result<Result<Vec<messaging::types::BrokerMessage>, String>> {
        let timeout = Duration::from_millis(timeout_ms.into());
        let max_results = max_results.try_into().unwrap_or(usize::MAX);
        let mut msgs = Vec::with_capacity(max_results);
        match self
            .handler
            .request_multi(subject, body, timeout, &mut msgs)
            .await
        {
            Ok(n) if n <= max_results && n == msgs.len() => Ok(Ok(msgs)),
            Ok(_) => bail!("invalid amount of results returned"),
            Err(err) => Ok(Err(format!("{err:#}"))),
        }
    }

    #[instrument]
    async fn publish(
        &mut self,
        msg: messaging::types::BrokerMessage,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(self
            .handler
            .publish(msg)
            .await
            .map_err(|err| format!("{err:#}")))
    }
}

#[async_trait]
impl blobstore::types::Host for Ctx {}

#[allow(unused)] // TODO: Remove once implemented
#[async_trait]
impl blobstore::consumer::Host for Ctx {
    #[instrument]
    async fn container_exists(&mut self, container_id: String) -> anyhow::Result<bool> {
        bail!("unsupported")
    }

    #[instrument]
    async fn create_container(
        &mut self,
        container_id: String,
    ) -> anyhow::Result<Result<(), String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn remove_container(
        &mut self,
        container_id: String,
    ) -> anyhow::Result<Result<(), String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn get_container_info(
        &mut self,
        container_id: String,
    ) -> anyhow::Result<Result<Option<blobstore::types::ContainerInfo>, String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn get_object_info(
        &mut self,
        container_id: String,
        object_id: String,
    ) -> anyhow::Result<Result<Option<blobstore::types::ObjectInfo>, String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn remove_object(
        &mut self,
        container_id: String,
        object_id: String,
    ) -> anyhow::Result<Result<bool, String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn put_object(
        &mut self,
        chunk: blobstore::types::Chunk,
        content_type: String,
        content_encoding: String,
    ) -> anyhow::Result<Result<String, String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn put_chunk(
        &mut self,
        stream_id: String,
        chunk: blobstore::types::Chunk,
        cancel: bool,
    ) -> anyhow::Result<Result<(), String>> {
        bail!("unsupported")
    }

    #[instrument]
    async fn stream_object(
        &mut self,
        container_id: String,
        object_id: String,
    ) -> anyhow::Result<Result<(), String>> {
        bail!("unsupported")
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

    /// Set [`Bus`] handler for this [Instance].
    pub fn bus(&mut self, bus: Arc<dyn Bus + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_bus(bus);
        self
    }

    /// Set [`IncomingHttp`] handler for this [Instance].
    pub fn incoming_http(
        &mut self,
        incoming_http: Arc<dyn IncomingHttp + Send + Sync>,
    ) -> &mut Self {
        self.handler_mut().replace_incoming_http(incoming_http);
        self
    }

    /// Set [`Logging`] handler for this [Instance].
    pub fn logging(&mut self, logging: Arc<dyn Logging + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_logging(logging);
        self
    }

    /// Set [`Messaging`] handler for this [Instance].
    pub fn messaging(&mut self, messaging: Arc<dyn Messaging + Send + Sync>) -> &mut Self {
        self.handler_mut().replace_messaging(messaging);
        self
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

    /// Instantiates and returns a [`InterfaceInstance<incoming_http_bindings::IncomingHttp>`] if exported by the [`Instance`].
    ///
    /// # Errors
    ///
    /// Fails if incoming HTTP bindings are not exported by the [`Instance`]
    pub async fn into_incoming_http(
        mut self,
    ) -> anyhow::Result<InterfaceInstance<incoming_http_bindings::IncomingHttp>> {
        let bindings = if let Ok((bindings, _)) =
            incoming_http_bindings::IncomingHttp::instantiate_async(
                &mut self.store,
                &self.component,
                &self.linker,
            )
            .await
        {
            InterfaceBindings::Interface(bindings)
        } else {
            self.as_guest_bindings()
                .await
                .map(InterfaceBindings::Guest)
                .context("failed to instantiate `wasi:http/incoming-handler` interface")?
        };
        Ok(InterfaceInstance {
            store: Mutex::new(self.store),
            bindings,
        })
    }

    /// Instantiates and returns an [`InterfaceInstance<logging_bindings::Logging>`] if exported by the [`Instance`].
    ///
    /// # Errors
    ///
    /// Fails if logging bindings are not exported by the [`Instance`]
    pub async fn into_logging(
        mut self,
    ) -> anyhow::Result<InterfaceInstance<logging_bindings::Logging>> {
        let bindings = if let Ok((bindings, _)) = logging_bindings::Logging::instantiate_async(
            &mut self.store,
            &self.component,
            &self.linker,
        )
        .await
        {
            InterfaceBindings::Interface(bindings)
        } else {
            self.as_guest_bindings()
                .await
                .map(InterfaceBindings::Guest)
                .context("failed to instantiate `wasi:logging/logging` interface")?
        };
        Ok(InterfaceInstance {
            store: Mutex::new(self.store),
            bindings,
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

#[async_trait]
impl Logging for InterfaceInstance<logging_bindings::Logging> {
    #[instrument(skip(self))]
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        let mut store = self.store.lock().await;
        match &self.bindings {
            InterfaceBindings::Guest(guest) => {
                let level = match level {
                    logging::Level::Trace => "trace",
                    logging::Level::Debug => "debug",
                    logging::Level::Info => "info",
                    logging::Level::Warn => "warn",
                    logging::Level::Error => "error",
                    logging::Level::Critical => "critical",
                };
                let request = serde_json::to_vec(&json!({
                    "level": level,
                    "context": context,
                    "message": message,
                }))
                .context("failed to encode request")?;
                guest
                    .call(
                        &mut store,
                        "wasi:logging/logging.log",
                        Cursor::new(request),
                        sink(),
                    )
                    .await
                    .context("failed to call actor")?
                    .map_err(|e| anyhow!(e))
            }
            InterfaceBindings::Interface(bindings) => {
                // NOTE: It appears that unifying the `Level` type is not possible currently
                use logging_bindings::exports::wasi::logging::logging::Level;
                let level = match level {
                    logging::Level::Trace => Level::Trace,
                    logging::Level::Debug => Level::Debug,
                    logging::Level::Info => Level::Info,
                    logging::Level::Warn => Level::Warn,
                    logging::Level::Error => Level::Error,
                    logging::Level::Critical => Level::Critical,
                };
                trace!("call `wasi:logging/logging.log`");
                bindings
                    .wasi_logging_logging()
                    .call_log(&mut *store, level, &context, &message)
                    .await
            }
        }
    }
}

#[async_trait]
impl IncomingHttp for InterfaceInstance<incoming_http_bindings::IncomingHttp> {
    #[allow(unused)] // TODO: Remove
    #[instrument(skip_all)]
    async fn handle(
        &self,
        request: http::Request<Box<dyn AsyncRead + Sync + Send + Unpin>>,
    ) -> anyhow::Result<http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>> {
        let (
            http::request::Parts {
                method,
                uri,
                headers,
                ..
            },
            body,
        ) = request.into_parts();
        let path_with_query = uri.path_and_query().map(http::uri::PathAndQuery::as_str);
        let scheme = uri.scheme_str();
        let authority = uri.authority().map(http::uri::Authority::as_str);
        let mut store = self.store.lock().await;
        bail!("unsupported"); // TODO: Support
    }
}
