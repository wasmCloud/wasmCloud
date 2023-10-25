use crate::actor::claims;
use crate::capability::{builtin, Bus, Interfaces, TargetInterface};
use crate::Runtime;

use core::fmt::{self, Debug};
use core::mem::replace;
use core::ops::{Deref, DerefMut};

use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context as _};
use async_trait::async_trait;
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;
use tracing::{error, instrument, trace};
use wascap::jwt;
use wasmtime::component::{Linker, Val};
use wasmtime_wasi::preview2::command::{self, Command};
use wasmtime_wasi::preview2::pipe::{
    AsyncReadStream, AsyncWriteStream, ClosedInputStream, ClosedOutputStream,
};
use wasmtime_wasi::preview2::{
    HostInputStream, HostOutputStream, StdinStream, StdoutStream, StreamError, StreamResult,
    Subscribe, Table, TableError, WasiCtx, WasiCtxBuilder, WasiView,
};
use wasmtime_wasi_http::WasiHttpCtx;
use wit_parser::{Results, Type, World, WorldId, WorldKey};

mod blobstore;
mod bus;
mod http;
mod keyvalue;
mod logging;
mod messaging;

pub(crate) use self::http::incoming_http_bindings;
pub(crate) use self::logging::logging_bindings;

type TableResult<T> = Result<T, TableError>;

mod guest_bindings {
    wasmtime::component::bindgen!({
        world: "guest",
        async: true,
        with: {
           "wasi:io/error@0.2.0-rc-2023-11-10": wasmtime_wasi::preview2::bindings::io::error,
           "wasi:io/poll@0.2.0-rc-2023-11-10": wasmtime_wasi::preview2::bindings::io::poll,
           "wasi:io/streams@0.2.0-rc-2023-11-10": wasmtime_wasi::preview2::bindings::io::streams,
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

impl HostInputStream for StdioStream<Box<dyn HostInputStream>> {
    #[instrument(level = "trace", skip(self))]
    fn read(&mut self, size: usize) -> StreamResult<Bytes> {
        match self.0.try_lock().as_deref_mut() {
            Ok(None) => ClosedInputStream.read(size),
            Ok(Some(stream)) => stream.read(size),
            Err(_) => Ok(Bytes::default()),
        }
    }

    #[instrument(level = "trace", skip(self))]
    fn skip(&mut self, nelem: usize) -> StreamResult<usize> {
        match self.0.try_lock().as_deref_mut() {
            Ok(None) => ClosedInputStream.skip(nelem),
            Ok(Some(stream)) => stream.skip(nelem),
            Err(_) => Ok(0),
        }
    }
}

#[async_trait]
impl Subscribe for StdioStream<Box<dyn HostInputStream>> {
    #[instrument(level = "trace", skip(self))]
    async fn ready(&mut self) {
        if let Some(stream) = self.0.lock().await.as_mut() {
            stream.ready().await;
        } else {
            ClosedInputStream.ready().await;
        }
    }
}

impl StdinStream for StdioStream<Box<dyn HostInputStream>> {
    fn stream(&self) -> Box<dyn HostInputStream> {
        Box::new(self.clone())
    }

    fn isatty(&self) -> bool {
        false
    }
}

#[async_trait]
impl HostOutputStream for StdioStream<Box<dyn HostOutputStream>> {
    #[instrument(level = "trace", skip(self))]
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        match self.0.try_lock().as_deref_mut() {
            Ok(None) => ClosedOutputStream.write(bytes),
            Ok(Some(stream)) => stream.write(bytes),
            Err(_) => Err(StreamError::Trap(anyhow!("deadlock"))),
        }
    }

    #[instrument(level = "trace", skip(self))]
    fn write_zeroes(&mut self, nelem: usize) -> StreamResult<()> {
        match self.0.try_lock().as_deref_mut() {
            Ok(None) => ClosedOutputStream.write_zeroes(nelem),
            Ok(Some(stream)) => stream.write_zeroes(nelem),
            Err(_) => Err(StreamError::Trap(anyhow!("deadlock"))),
        }
    }

    #[instrument(level = "trace", skip(self))]
    fn flush(&mut self) -> StreamResult<()> {
        match self.0.try_lock().as_deref_mut() {
            Ok(None) => ClosedOutputStream.flush(),
            Ok(Some(stream)) => stream.flush(),
            Err(_) => Err(StreamError::Trap(anyhow!("deadlock"))),
        }
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        match self.0.try_lock().as_deref_mut() {
            Ok(None) => ClosedOutputStream.check_write(),
            Ok(Some(stream)) => stream.check_write(),
            Err(_) => Err(StreamError::Trap(anyhow!("deadlock"))),
        }
    }
}

#[async_trait]
impl Subscribe for StdioStream<Box<dyn HostOutputStream>> {
    #[instrument(level = "trace", skip(self))]
    async fn ready(&mut self) {
        if let Some(stream) = self.0.lock().await.as_mut() {
            stream.ready().await;
        } else {
            ClosedOutputStream.ready().await;
        }
    }
}

impl StdoutStream for StdioStream<Box<dyn HostOutputStream>> {
    fn stream(&self) -> Box<dyn HostOutputStream> {
        Box::new(self.clone())
    }

    fn isatty(&self) -> bool {
        false
    }
}

struct Ctx {
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: Table,
    handler: builtin::Handler,
    stdin: StdioStream<Box<dyn HostInputStream>>,
    stdout: StdioStream<Box<dyn HostOutputStream>>,
    stderr: StdioStream<Box<dyn HostOutputStream>>,
}

impl WasiView for Ctx {
    fn table(&self) -> &Table {
        &self.table
    }

    fn table_mut(&mut self) -> &mut Table {
        &mut self.table
    }

    fn ctx(&self) -> &WasiCtx {
        &self.wasi
    }

    fn ctx_mut(&mut self) -> &mut WasiCtx {
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
    linker: Linker<Ctx>,
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

fn encode_custom_parameters(params: &[Val]) -> anyhow::Result<Vec<u8>> {
    match params {
        [] => Ok(vec![]),
        [Val::Bool(val)] => rmp_serde::to_vec(val),
        [Val::S8(val)] => rmp_serde::to_vec(val),
        [Val::U8(val)] => rmp_serde::to_vec(val),
        [Val::S16(val)] => rmp_serde::to_vec(val),
        [Val::U16(val)] => rmp_serde::to_vec(val),
        [Val::S32(val)] => rmp_serde::to_vec(val),
        [Val::U32(val)] => rmp_serde::to_vec(val),
        [Val::S64(val)] => rmp_serde::to_vec(val),
        [Val::U64(val)] => rmp_serde::to_vec(val),
        [Val::Float32(val)] => rmp_serde::to_vec(val),
        [Val::Float64(val)] => rmp_serde::to_vec(val),
        [Val::Char(val)] => rmp_serde::to_vec(val),
        [Val::String(val)] => rmp_serde::to_vec(val),
        _ => bail!("complex types not supported yet"),
    }
    .context("failed to encode parameters")
}

fn decode_custom_results(
    results_ty: &Results,
    results: &mut [Val],
    buf: &[u8],
) -> anyhow::Result<()> {
    let mut results_ty = results_ty.iter_types();
    match (results_ty.next(), results_ty.next(), results) {
        (None, None, []) => {
            ensure!(
                buf.is_empty(),
                "non-empty response returned when none expected"
            );
        }
        (Some(Type::Bool), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::Bool)?;
        }
        (Some(Type::U8), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::U8)?;
        }
        (Some(Type::U16), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::U16)?;
        }
        (Some(Type::U32), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::U32)?;
        }
        (Some(Type::U64), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::U64)?;
        }
        (Some(Type::S8), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::S8)?;
        }
        (Some(Type::S16), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::S16)?;
        }
        (Some(Type::S32), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::S32)?;
        }
        (Some(Type::S64), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::S64)?;
        }
        (Some(Type::Float32), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::Float32)?;
        }
        (Some(Type::Float64), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::Float64)?;
        }
        (Some(Type::Char), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::Char)?;
        }
        (Some(Type::String), None, [val]) => {
            *val = rmp_serde::from_slice(buf)
                .context("failed to decode response")
                .map(Val::String)?;
        }
        _ => bail!("complex types not supported yet"),
    }
    Ok(())
}

#[instrument(level = "trace", skip_all)]
fn wasifill(
    component: &wasmtime::component::Component,
    resolve: &wit_parser::Resolve,
    world: WorldId,
    linker: &mut Linker<Ctx>,
) {
    let Some(World { imports, .. }) = resolve
        .worlds
        .iter()
        .find_map(|(id, w)| (id == world).then_some(w))
    else {
        trace!("component world missing");
        return;
    };
    for (key, _) in imports {
        let WorldKey::Interface(iface) = key else {
            continue;
        };
        let Some(interface) = resolve.interfaces.get(*iface) else {
            trace!("component imports a non-existent interface");
            continue;
        };
        let Some(ref interface_name) = interface.name else {
            trace!("component imports an unnamed interface");
            continue;
        };
        let Some(package) = interface.package else {
            trace!(
                interface = interface_name,
                "component interface import is missing a package"
            );
            continue;
        };
        let Some(package) = resolve.packages.get(package) else {
            trace!(
                interface = interface_name,
                "component interface belongs to a non-existent package"
            );
            continue;
        };
        match (package.name.namespace.as_str(), package.name.name.as_str()) {
            (
                "wasi",
                "blobstore" | "cli" | "clocks" | "filesystem" | "http" | "io" | "keyvalue"
                | "logging" | "random" | "sockets",
            )
            | ("wasmcloud", "bus" | "messaging") => continue,
            _ => {
                let interface_path = format!("{}/{interface_name}", package.name);
                let mut linker = linker.root();
                let mut linker = match linker.instance(&interface_path) {
                    Ok(linker) => linker,
                    Err(err) => {
                        error!(
                            ?err,
                            namespace = package.name.namespace,
                            "failed to instantiate interface from root"
                        );
                        continue;
                    }
                };
                let target = Arc::new(TargetInterface::Custom {
                    namespace: package.name.namespace.clone(),
                    package: package.name.name.clone(),
                    interface: interface_name.to_string(),
                });
                for (name, function) in interface.functions.iter().filter(|(name, function)| {
                    if function.params.len() > 1
                        || function.results.len() > 1
                        || function
                            .params
                            .iter()
                            .any(|(_, ty)| matches!(ty, Type::Id(_)))
                        || function
                            .results
                            .iter_types()
                            .any(|ty| matches!(ty, Type::Id(_)))
                    {
                        trace!(
                            namespace = package.name.namespace,
                            package = package.name.name,
                            interface = interface_name,
                            name,
                            "avoid wasifilling unsupported component function import"
                        );
                        false
                    } else {
                        true
                    }
                }) {
                    trace!(
                        namespace = package.name.namespace,
                        package = package.name.name,
                        interface = interface_name,
                        name,
                        "wasifill component function import"
                    );
                    let operation = format!("{interface_path}.{name}");
                    let target = Arc::clone(&target);
                    let results_ty = Arc::new(function.results.clone());
                    if let Err(err) =
                        linker.func_new_async(component, name, move |ctx, params, results| {
                            let operation = operation.clone();
                            let target = Arc::clone(&target);
                            let results_ty = Arc::clone(&results_ty);
                            Box::new(async move {
                                let buf = encode_custom_parameters(params)?;
                                let handler = &ctx.data().handler;
                                let target = handler
                                    .identify_interface_target(&target)
                                    .await
                                    .context("failed to identify interface target")?;
                                let buf = handler
                                    .call_sync(target, operation, buf)
                                    .await
                                    .context("failed to call target")?;
                                decode_custom_results(&results_ty, results, &buf)
                            })
                        })
                    {
                        error!(
                            ?err,
                            namespace = package.name.namespace,
                            package = package.name.name,
                            interface = interface_name,
                            name,
                            "failed to wasifill component function import"
                        );
                    }
                }
            }
        }
    }
}

#[instrument(level = "trace", skip_all)]
fn instantiate(
    component: wasmtime::component::Component,
    engine: &wasmtime::Engine,
    linker: Linker<Ctx>,
    handler: impl Into<builtin::Handler>,
) -> anyhow::Result<Instance> {
    let stdin = StdioStream::default();
    let stdout = StdioStream::default();
    let stderr = StdioStream::default();

    let table = Table::new();
    let wasi = WasiCtxBuilder::new()
        .args(&["main.wasm"]) // TODO: Configure argv[0]
        .stdin(stdin.clone())
        .stdout(stdout.clone())
        .stderr(stderr.clone())
        .build();
    let handler = handler.into();
    let ctx = Ctx {
        wasi,
        http: WasiHttpCtx,
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
        let (resolve, world) =
            match wit_component::decode(wasm).context("failed to decode WIT component")? {
                wit_component::DecodedWasm::Component(resolve, world) => (resolve, world),
                wit_component::DecodedWasm::WitPackage(..) => {
                    bail!("binary-encoded WIT packages not supported")
                }
            };
        let claims = claims(wasm)?;
        let component = wasmtime::component::Component::new(&engine, wasm)
            .context("failed to compile component")?;

        let mut linker = Linker::new(&engine);

        Interfaces::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasmcloud:host/interfaces` interface")?;
        wasmtime_wasi_http::bindings::wasi::http::types::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasi:http/types` interface")?;
        wasmtime_wasi_http::bindings::wasi::http::outgoing_handler::add_to_linker(
            &mut linker,
            |ctx| ctx,
        )
        .context("failed to link `wasi:http/outgoing-handler` interface")?;

        command::add_to_linker(&mut linker).context("failed to link core WASI interfaces")?;

        wasifill(&component, &resolve, world, &mut linker);

        Ok(Self {
            component,
            engine,
            linker,
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
        let instance = instantiate(self.component, &self.engine, self.linker, self.handler)?;
        Ok((instance, self.claims))
    }

    /// Instantiates a [Component] and returns the resulting [Instance].
    #[instrument]
    pub fn instantiate(&self) -> anyhow::Result<Instance> {
        instantiate(
            self.component.clone(),
            &self.engine,
            self.linker.clone(),
            self.handler.clone(),
        )
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
                let wasi = WasiCtxBuilder::new()
                    .args(&["main.wasm", operation]) // TODO: Configure argv[0]
                    .stdin(ctx.stdin.clone())
                    .stdout(ctx.stdout.clone())
                    .stderr(ctx.stderr.clone())
                    .build();
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
