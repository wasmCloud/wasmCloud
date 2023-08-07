mod wasmbus;

use wasmbus::guest_call;

use crate::actor::claims;
use crate::capability::logging::logging;
use crate::capability::{builtin, Bus, IncomingHttp, Logging, Messaging};
use crate::Runtime;

use core::any::Any;
use core::fmt::{self, Debug};

use std::io::Cursor;
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context};
use async_trait::async_trait;
use futures::lock::Mutex;
use serde_json::json;
use tokio::io::{sink, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::runtime::Handle;
use tokio::task;
use tracing::{instrument, trace};
use wascap::jwt;
use wasi_common::file::{FdFlags, FileType};
use wasi_common::pipe::WritePipe;
use wasmtime::TypedFunc;
use wasmtime_wasi::{WasiCtxBuilder, WasiFile};

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

struct AsyncReadPipe<T>(Arc<Mutex<T>>);

#[async_trait]
impl<T: AsyncRead + Sync + Send + Unpin + 'static> WasiFile for AsyncReadPipe<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }
    async fn get_filetype(&self) -> Result<FileType, wasi_common::Error> {
        Ok(FileType::Pipe)
    }
    async fn read_vectored<'a>(
        &self,
        bufs: &mut [std::io::IoSliceMut<'a>],
    ) -> Result<u64, wasi_common::Error> {
        task::block_in_place(move || {
            Handle::current().block_on(async {
                let mut stream = self.0.lock().await;
                for buf in bufs {
                    if buf.len() == 0 {
                        continue;
                    }
                    let n = stream.read(buf).await?;
                    let n = n.try_into()?;
                    return Ok(n);
                }
                Ok(0)
            })
        })
    }

    async fn readable(&self) -> Result<(), wasi_common::Error> {
        Ok(())
    }
}

struct AsyncWritePipe<T>(Arc<Mutex<T>>);

#[async_trait]
impl<T: AsyncWrite + Sync + Send + Unpin + 'static> WasiFile for AsyncWritePipe<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }
    async fn get_filetype(&self) -> Result<FileType, wasi_common::Error> {
        Ok(FileType::Pipe)
    }
    async fn get_fdflags(&self) -> Result<FdFlags, wasi_common::Error> {
        Ok(FdFlags::APPEND)
    }
    async fn write_vectored<'a>(
        &self,
        bufs: &[std::io::IoSlice<'a>],
    ) -> Result<u64, wasi_common::Error> {
        task::block_in_place(move || {
            Handle::current().block_on(async {
                let n = self.0.lock().await.write_vectored(bufs).await?;
                let n = n.try_into()?;
                Ok(n)
            })
        })
    }

    async fn writable(&self) -> Result<(), wasi_common::Error> {
        Ok(())
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
    claims: Option<jwt::Claims<jwt::Actor>>,
    config: Config,
    handler: builtin::HandlerBuilder,
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
    pub fn claims(&self) -> Option<&jwt::Claims<jwt::Actor>> {
        self.claims.as_ref()
    }
}

async fn instantiate(
    module: &wasmtime::Module,
    config: &Config,
    handler: impl Into<builtin::Handler>,
) -> anyhow::Result<Instance> {
    let wasi = WasiCtxBuilder::new();
    let wasi = wasi
        .arg("main.wasm")
        .context("failed to set argv[0]")?
        .build();
    let ctx = Ctx {
        wasi,
        wasmbus: wasmbus::Ctx::new(handler),
    };

    let engine = module.engine();

    let mut store = wasmtime::Store::new(engine, ctx);
    let mut linker = wasmtime::Linker::<Ctx>::new(engine);

    wasmtime_wasi::add_to_linker(&mut linker, |ctx| &mut ctx.wasi)
        .context("failed to link WASI")?;
    wasmbus::add_to_linker(&mut linker).context("failed to link wasmbus")?;

    let memory = wasmtime::Memory::new(
        &mut store,
        wasmtime::MemoryType::new(config.min_memory_pages, config.max_memory_pages),
    )
    .context("failed to initialize memory")?;
    linker
        .define_name(&store, "memory", memory)
        .context("failed to define `memory`")?;

    let instance = linker
        .instantiate_async(&mut store, module)
        .await
        .context("failed to instantiate module")?;
    let start = instance.get_typed_func(&mut store, "_start");
    let guest_call = instance
        .get_typed_func::<guest_call::Params, guest_call::Result>(&mut store, "__guest_call");
    let (start, guest_call) = match (start, guest_call) {
        (Ok(start), Ok(guest_call)) => (Some(start), Some(guest_call)),
        (Ok(start), Err(_)) => (Some(start), None),
        (Err(_), Ok(guest_call)) => (None, Some(guest_call)),
        (Err(_), Err(e)) => {
            bail!("failed to instantiate either  `_start`, or `__guest_call`: {e}")
        }
    };
    Ok(Instance {
        store,
        guest_call,
        start,
    })
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

    /// Like [Self::instantiate], but moves the [Module].
    #[instrument]
    pub async fn into_instance(self) -> anyhow::Result<Instance> {
        instantiate(&self.module, &self.config, self.handler).await
    }

    /// Like [Self::instantiate], but moves the [Module] and returns the associated [jwt::Claims].
    #[instrument]
    pub async fn into_instance_claims(
        self,
    ) -> anyhow::Result<(Instance, Option<jwt::Claims<jwt::Actor>>)> {
        let instance = instantiate(&self.module, &self.config, self.handler).await?;
        Ok((instance, self.claims))
    }

    /// Instantiates a [Module] and returns the resulting [Instance].
    #[instrument]
    pub async fn instantiate(&self) -> anyhow::Result<Instance> {
        instantiate(&self.module, &self.config, self.handler.clone()).await
    }

    /// Instantiate a [Module] producing an [Instance] and invoke an operation on it using [Instance::call]
    #[instrument(skip_all)]
    pub async fn call(
        &self,
        operation: impl AsRef<str>,
        request: impl AsyncRead + Send + Sync + Unpin + 'static,
        response: impl AsyncWrite + Send + Sync + Unpin + 'static,
    ) -> anyhow::Result<Result<(), String>> {
        self.instantiate()
            .await
            .context("failed to instantiate module")?
            .call(operation, request, response)
            .await
    }
}

/// An instance of a [Module]
pub struct Instance {
    store: wasmtime::Store<Ctx>,
    guest_call: Option<TypedFunc<guest_call::Params, guest_call::Result>>,
    start: Option<TypedFunc<(), ()>>,
}

impl Debug for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Instance")
            .field("runtime", &"wasmtime")
            .field("guest_call", &self.guest_call.is_some().to_string())
            .field("start", &self.start.is_some().to_string())
            .finish()
    }
}

impl Instance {
    /// Returns a mutable reference to embedded [`builtin::Handler`]
    fn handler_mut(&mut self) -> &mut builtin::Handler {
        &mut self.store.data_mut().wasmbus.handler
    }

    /// Reset [`Instance`] state to defaults
    pub fn reset(&mut self, rt: &Runtime) {
        *self.handler_mut() = rt.handler.clone().into();
        self.store
            .data_mut()
            .wasi
            .set_stderr(Box::new(WritePipe::new(std::io::sink())));
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

    /// Set actor stderr stream. If another stderr was set, it is replaced.
    pub fn stderr(&mut self, stderr: impl AsyncWrite + Send + Sync + Unpin + 'static) -> &mut Self {
        let stderr = AsyncWritePipe(Arc::new(Mutex::new(stderr)));
        self.store.data_mut().wasi.set_stderr(Box::new(stderr));
        self
    }

    /// Invoke an operation on an [Instance].
    #[instrument(skip_all)]
    pub async fn call(
        &mut self,
        operation: impl AsRef<str>,
        mut request: impl AsyncRead + Send + Sync + Unpin + 'static,
        mut response: impl AsyncWrite + Send + Sync + Unpin + 'static,
    ) -> anyhow::Result<Result<(), String>> {
        self.store.data_mut().reset();

        // TODO: Introduce wasmbus v2 with two-way streaming

        let guest_call = match (self.start, self.guest_call) {
            (Some(start), Some(guest_call)) => {
                trace!("call `_start`");
                start
                    .call_async(&mut self.store, ())
                    .await
                    .context("failed to call `_start`")?;
                guest_call
            }
            (Some(start), None) => {
                // TODO: The argument vector here should be replaced, but that is not possible
                // currently due to Wasmtime limitations
                self.store
                    .data_mut()
                    .wasi
                    .push_arg(operation.as_ref())
                    .context("failed to push arg")?;
                let stdin = AsyncReadPipe(Arc::new(Mutex::new(request)));
                let stdout = AsyncWritePipe(Arc::new(Mutex::new(response)));
                self.store.data_mut().wasi.set_stdin(Box::new(stdin));
                self.store.data_mut().wasi.set_stdout(Box::new(stdout));
                trace!("call `_start`");
                start
                    .call_async(&mut self.store, ())
                    .await
                    .context("failed to call `_start`")?;
                return Ok(Ok(()));
            }
            (None, Some(guest_call)) => guest_call,
            (None, None) => {
                bail!("failed to call either  `_start`, or `__guest_call`")
            }
        };

        let operation = operation.as_ref();
        let operation_len = operation
            .len()
            .try_into()
            .context("operation string length does not fit in u32")?;

        let mut payload = vec![];
        let payload_len = request
            .read_to_end(&mut payload)
            .await
            .context("failed to read payload")?
            .try_into()
            .context("payload length does not fit in u32")?;

        self.store
            .data_mut()
            .wasmbus
            .set_guest_call(operation.to_string(), payload);

        trace!("call `_guest_call`");
        let code = guest_call
            .call_async(&mut self.store, (operation_len, payload_len))
            .await
            .context("failed to call `__guest_call`")?;
        if let Some(err) = self.store.data_mut().wasmbus.take_guest_error() {
            return Ok(Err(err));
        } else if let Some(err) = self.store.data_mut().wasmbus.take_host_error() {
            return Ok(Err(err));
        }
        let res = self.store.data_mut().wasmbus.take_guest_response();
        let console_log: Vec<_> = self.store.data_mut().wasmbus.take_console_log();
        ensure!(code == 1, "operation failed with exit code `{code}`");
        if !console_log.is_empty() {
            trace!(?console_log);
        }
        if let Some(res) = res {
            response
                .write_all(&res)
                .await
                .context("failed to write response")?;
        }
        Ok(Ok(()))
    }
}

/// Instantiated, clone-able guest instance
#[derive(Clone, Debug)]
pub struct GuestInstance(Arc<Mutex<Instance>>);

impl From<Instance> for GuestInstance {
    fn from(instance: Instance) -> Self {
        Self(Arc::new(instance.into()))
    }
}

impl GuestInstance {
    /// Invoke an operation on a [GuestInstance].
    #[instrument(skip_all)]
    pub async fn call(
        &self,
        operation: impl AsRef<str>,
        request: impl AsyncRead + Send + Sync + Unpin + 'static,
        response: impl AsyncWrite + Send + Sync + Unpin + 'static,
    ) -> anyhow::Result<Result<(), String>> {
        self.0.lock().await.call(operation, request, response).await
    }
}

#[async_trait]
impl Logging for GuestInstance {
    #[instrument(skip(self))]
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
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
        self.call("wasi:logging/logging.log", Cursor::new(request), sink())
            .await
            .context("failed to call actor")?
            .map_err(|e| anyhow!(e))
    }
}

#[async_trait]
impl IncomingHttp for GuestInstance {
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
        self.0.lock().await;
        bail!("unsupported"); // TODO: Support
    }
}
