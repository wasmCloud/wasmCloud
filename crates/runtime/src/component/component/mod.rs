use crate::capability::builtin;
use crate::capability::builtin::Handler;
use crate::component::claims;
use crate::Runtime;

use core::fmt::{self, Debug};
use core::iter::zip;
use core::ops::{Deref, DerefMut};
use core::pin::pin;
use core::time::Duration;

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::TryStreamExt;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::Mutex;
use tokio_util::codec::{Encoder, FramedRead};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{error, instrument, trace, warn};
use wascap::jwt;
use wasm_tokio::cm::AsyncReadValue as _;
use wasm_tokio::{AsyncReadCore as _, AsyncReadLeb128 as _, AsyncReadUtf8 as _};
use wasmcloud_component_adapters::WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER;
use wasmcloud_core::CallTargetInterface;
use wasmtime::component::types::{Case, Field};
use wasmtime::component::{
    self, types, InstancePre, Linker, ResourceTable, ResourceTableError, ResourceType, Type, Val,
};
use wasmtime::AsContextMut;
use wasmtime_wasi::pipe::{
    AsyncReadStream, AsyncWriteStream, ClosedInputStream, ClosedOutputStream,
};
use wasmtime_wasi::{
    HostInputStream, HostOutputStream, InputStream, StdinStream, StdoutStream, StreamError,
    StreamResult, Subscribe, WasiCtx, WasiCtxBuilder, WasiView,
};
use wasmtime_wasi_http::WasiHttpCtx;
use wrpc_runtime_wasmtime::{RemoteResource, ValEncoder, WrpcView};
use wrpc_transport::{Invoke, ListDecoderU8};
use wrpc_types::{function_exports, DynamicFunction};

mod blobstore;
mod bus;
mod config;
mod http;
mod keyvalue;
mod logging;
mod messaging;

/// skips instance names, for which static (builtin) bindings exist
macro_rules! skip_static_instances {
    ($instance:expr) => {
        match ($instance) {
            "wasi:blobstore/blobstore@0.2.0-draft"
            | "wasi:blobstore/container@0.2.0-draft"
            | "wasi:blobstore/types@0.2.0-draft"
            | "wasi:cli/environment@0.2.0"
            | "wasi:cli/exit@0.2.0"
            | "wasi:cli/stderr@0.2.0"
            | "wasi:cli/stdin@0.2.0"
            | "wasi:cli/stdout@0.2.0"
            | "wasi:cli/terminal-input@0.2.0"
            | "wasi:cli/terminal-output@0.2.0"
            | "wasi:cli/terminal-stderr@0.2.0"
            | "wasi:cli/terminal-stdin@0.2.0"
            | "wasi:cli/terminal-stdout@0.2.0"
            | "wasi:clocks/monotonic-clock@0.2.0"
            | "wasi:clocks/wall-clock@0.2.0"
            | "wasi:config/runtime@0.2.0-draft"
            | "wasi:filesystem/preopens@0.2.0"
            | "wasi:filesystem/types@0.2.0"
            | "wasi:http/incoming-handler@0.2.0"
            | "wasi:http/outgoing-handler@0.2.0"
            | "wasi:http/types@0.2.0"
            | "wasi:io/error@0.2.0"
            | "wasi:io/poll@0.2.0"
            | "wasi:io/streams@0.2.0"
            | "wasi:keyvalue/atomics@0.2.0-draft"
            | "wasi:keyvalue/store@0.2.0-draft"
            | "wasi:logging/logging"
            | "wasi:random/random@0.2.0"
            | "wasi:sockets/instance-network@0.2.0"
            | "wasi:sockets/network@0.2.0"
            | "wasi:sockets/tcp-create-socket@0.2.0"
            | "wasi:sockets/tcp@0.2.0"
            | "wasi:sockets/udp-create-socket@0.2.0"
            | "wasi:sockets/udp@0.2.0"
            | "wasmcloud:bus/lattice@1.0.0"
            | "wasmcloud:messaging/consumer@0.2.0"
            | "wasmcloud:messaging/handler@0.2.0"
            | "wasmcloud:messaging/types@0.2.0" => continue,
            _ => {}
        }
    };
}

type TableResult<T> = Result<T, ResourceTableError>;

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
    table: ResourceTable,
    handler: builtin::Handler,
    stderr: StdioStream<Box<dyn HostOutputStream>>,
}

impl WrpcView<Handler> for Ctx {
    fn client(&self) -> &Handler {
        &self.handler
    }
}

impl WasiView for Ctx {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}

impl Debug for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx").field("runtime", &"wasmtime").finish()
    }
}

/// Pre-compiled component [Component], which is cheapily-[Cloneable](Clone)
#[derive(Clone)]
pub struct Component {
    engine: wasmtime::Engine,
    claims: Option<jwt::Claims<jwt::Component>>,
    handler: builtin::HandlerBuilder,
    exports: Arc<HashMap<String, HashMap<String, DynamicFunction>>>,
    ty: types::Component,
    instance_pre: wasmtime::component::InstancePre<Ctx>,
    max_execution_time: Duration,
}

impl Debug for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Component")
            .field("claims", &self.claims)
            .field("handler", &self.handler)
            .field("runtime", &"wasmtime")
            .field("exports", &self.exports)
            .field("ty", &self.ty)
            .field("max_execution_time", &self.max_execution_time)
            .finish_non_exhaustive()
    }
}

/// Polyfills all missing imports and returns instance -> function -> type map for each polyfill
#[instrument(level = "trace", skip_all)]
fn polyfill<'a, T>(
    resolve: &wit_parser::Resolve,
    imports: T,
    engine: &wasmtime::Engine,
    ty: &types::Component,
    linker: &mut Linker<Ctx>,
) -> ()
where
    T: IntoIterator<Item = (&'a wit_parser::WorldKey, &'a wit_parser::WorldItem)>,
    T::IntoIter: ExactSizeIterator,
{
    let imports = imports.into_iter();
    for (wk, item) in imports {
        let instance_name = resolve.name_world_key(wk);
        // Avoid polyfilling instances, for which static bindings are linked
        skip_static_instances!(instance_name.as_ref());
        let wit_parser::WorldItem::Interface(interface) = item else {
            continue;
        };
        let Some(wit_parser::Interface {
            name: interface_name,
            package,
            ..
        }) = resolve.interfaces.get(*interface)
        else {
            warn!("component imports a non-existent interface");
            continue;
        };
        let Some(interface_name) = interface_name else {
            trace!("component imports an unnamed interface");
            continue;
        };
        let Some(package) = package else {
            trace!(
                instance_name,
                "component interface import is missing a package"
            );
            continue;
        };
        let Some(wit_parser::Package {
            name: package_name, ..
        }) = resolve.packages.get(*package)
        else {
            trace!(
                instance_name,
                interface_name,
                "component interface belongs to a non-existent package"
            );
            continue;
        };
        let target = CallTargetInterface {
            namespace: package_name.namespace.to_string(),
            package: package_name.name.to_string(),
            interface: interface_name.to_string(),
        };
        let Some(types::ComponentItem::ComponentInstance(instance)) =
            ty.get_import(engine, &instance_name)
        else {
            trace!(
                instance_name,
                "component does not import the parsed instance"
            );
            continue;
        };

        let mut linker = linker.root();
        let mut linker = match linker.instance(&instance_name) {
            Ok(linker) => linker,
            Err(err) => {
                error!(
                    ?err,
                    ?instance_name,
                    "failed to instantiate interface from root"
                );
                continue;
            }
        };

        // No context gets put in here
        if let Err(err) = wrpc_runtime_wasmtime::link_instance(
            engine,
            &mut linker,
            instance,
            instance_name.clone(),
            target,
        ) {
            trace!(?err, ?instance_name, "failed to link instance");
            continue;
        }
    }
}

#[instrument(level = "trace", skip_all)]
fn instantiate(
    engine: &wasmtime::Engine,
    handler: impl Into<builtin::Handler>,
    ty: types::Component,
    instance_pre: InstancePre<Ctx>,
    max_execution_time: Duration,
) -> anyhow::Result<Instance> {
    let stdin = StdioStream::default();
    let stdout = StdioStream::default();
    let stderr = StdioStream::default();

    let table = ResourceTable::new();
    let wasi = WasiCtxBuilder::new()
        .args(&["main.wasm"]) // TODO: Configure argv[0]
        .stdin(stdin.clone())
        .stdout(stdout.clone())
        .stderr(stderr.clone())
        .build();

    let imports = ty.imports(engine);
    let mut polyfills = HashMap::with_capacity(imports.len());
    for (instance_name, item) in imports {
        // Skip static bindings, since the runtime types of their results are not needed by the
        // runtime - those will not be constructed using reflection, but rather directly returned
        // by Wasmtime
        skip_static_instances!(instance_name);
        let component::types::ComponentItem::ComponentInstance(item) = item else {
            continue;
        };
        let exports = item.exports(engine);
        let mut instance = HashMap::with_capacity(exports.len());
        for (func_name, item) in exports {
            let component::types::ComponentItem::ComponentFunc(ty) = item else {
                continue;
            };
            instance.insert(func_name.to_string(), ty);
        }
        if !instance.is_empty() {
            polyfills.insert(instance_name.to_string(), instance);
        }
    }

    let handler = handler.into();
    let ctx = Ctx {
        wasi,
        http: WasiHttpCtx::new(),
        table,
        handler,
        stderr,
    };
    let mut store = wasmtime::Store::new(engine, ctx);
    store.set_epoch_deadline(max_execution_time.as_secs());
    Ok(Instance {
        store,
        instance_pre,
    })
}

impl Component {
    /// Extracts [Claims](jwt::Claims) from WebAssembly component and compiles it using [Runtime].
    /// If `wasm` represents a core Wasm module, then it will first be turned into a component.
    #[instrument(level = "trace", skip_all)]
    pub fn new(rt: &Runtime, wasm: impl AsRef<[u8]>) -> anyhow::Result<Self> {
        let wasm = wasm.as_ref();
        if wasmparser::Parser::is_core_wasm(wasm) {
            let wasm = wit_component::ComponentEncoder::default()
                .module(wasm)
                .context("failed to set core component module")?
                .adapter(
                    "wasi_snapshot_preview1",
                    WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER,
                )
                .context("failed to add WASI preview1 adapter")?
                .encode()
                .context("failed to encode a component from module")?;
            return Self::new(rt, wasm);
        }
        let engine = rt.engine.clone();
        let claims = claims(wasm)?;
        let component = wasmtime::component::Component::new(&engine, wasm)
            .context("failed to compile component")?;

        let mut linker: Linker<Ctx> = Linker::new(&engine);

        wasmtime_wasi::add_to_linker_async(&mut linker)
            .context("failed to link core WASI interfaces")?;
        wasmtime_wasi_http::proxy::add_to_linker(&mut linker)
            .context("failed to link http proxy")?;

        // // Interfaces::add_to_linker(&mut linker, |ctx| &mut WasiImpl(ctx))
        // //     .context("failed to link `wasmcloud:host/interfaces` interface")?;
        // wasmtime_wasi_http::bindings::wasi::http::types::add_to_linker(&mut linker, |ctx| ctx)
        //     .context("failed to link `wasi:http/types` interface")?;
        // wasmtime_wasi_http::bindings::wasi::http::outgoing_handler::add_to_linker(
        //     &mut linker,
        //     |ctx| &mut WasiHttpImpl::new(ctx),
        // )
        // .context("failed to link `wasi:http/outgoing-handler` interface")?;

        let (resolve, world) =
            match wit_component::decode(wasm).context("failed to decode WIT component")? {
                wit_component::DecodedWasm::Component(resolve, world) => (resolve, world),
                wit_component::DecodedWasm::WitPackage(..) => {
                    bail!("binary-encoded WIT packages not currently supported")
                }
            };

        let wit_parser::World {
            exports, imports, ..
        } = resolve
            .worlds
            .iter()
            .find_map(|(id, w)| (id == world).then_some(w))
            .context("component world missing")?;

        let ty = component.component_type();
        polyfill(&resolve, imports, &engine, &ty, &mut linker);

        let linker = linker.to_owned();
        let instance_pre = linker.instantiate_pre(&component)?;
        // TODO: Record the substituted type exports, not parser exports
        Ok(Self {
            engine,
            claims,
            handler: rt.handler.clone(),
            exports: Arc::new(function_exports(&resolve, exports)),
            ty,
            instance_pre,
            max_execution_time: rt.max_execution_time,
        })
    }

    /// Sets maximum execution time for functionality exported by this component.
    /// Values below 1 second will be interpreted as 1 second.
    #[instrument(level = "trace", skip_all)]
    pub fn set_max_execution_time(&mut self, max_execution_time: Duration) -> &mut Self {
        self.max_execution_time = max_execution_time.max(Duration::from_secs(1));
        self
    }

    /// Reads the WebAssembly binary asynchronously and calls [Component::new].
    ///
    /// # Errors
    ///
    /// Fails if either reading `wasm` fails or [Self::new] fails
    #[instrument(skip(wasm))]
    pub async fn read(rt: &Runtime, mut wasm: impl AsyncRead + Unpin) -> anyhow::Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf)
            .await
            .context("failed to read Wasm")?;
        Self::new(rt, buf)
    }

    /// Reads the WebAssembly binary synchronously and calls [Component::new].
    ///
    /// # Errors
    ///
    /// Fails if either reading `wasm` fails or [Self::new] fails
    #[instrument(skip(wasm))]
    pub fn read_sync(rt: &Runtime, mut wasm: impl std::io::Read) -> anyhow::Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf).context("failed to read Wasm")?;
        Self::new(rt, buf)
    }

    /// Returns a map of dynamic function export types.
    /// Top level map is keyed by the instance name.
    /// Inner map is keyed by exported function name.
    #[must_use]
    pub fn exports(&self) -> &Arc<HashMap<String, HashMap<String, DynamicFunction>>> {
        &self.exports
    }

    // /// Returns a map of dynamic polyfilled function import types.
    // /// Top level map is keyed by the instance name.
    // /// Inner map is keyed by exported function name.
    // #[must_use]
    // pub fn polyfills(&self) -> &Arc<HashMap<String, HashMap<String, DynamicFunction>>> {
    //     &self.polyfills
    // }

    /// [Claims](jwt::Claims) associated with this [Component].
    #[instrument(level = "trace")]
    pub fn claims(&self) -> Option<&jwt::Claims<jwt::Component>> {
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
    ) -> anyhow::Result<(Instance, Option<jwt::Claims<jwt::Component>>)> {
        let instance = instantiate(
            &self.engine,
            self.handler,
            self.ty,
            self.instance_pre,
            self.max_execution_time,
        )?;
        Ok((instance, self.claims))
    }

    /// Instantiates a [Component] and returns the resulting [Instance].
    #[instrument(level = "debug", skip(self))]
    pub fn instantiate(&self) -> anyhow::Result<Instance> {
        instantiate(
            &self.engine,
            self.handler.clone(),
            self.ty.clone(),
            self.instance_pre.clone(),
            self.max_execution_time,
        )
    }

    /// Instantiates a [Component] producing an [Instance] and invokes an operation on it using [Instance::call]
    #[instrument(level = "trace", skip_all)]
    pub async fn call<C>(
        &self,
        instance: &str,
        name: &str,
        incoming: C::Incoming,
        outgoing: C::Outgoing,
    ) -> anyhow::Result<()>
    where
        C: wrpc_transport::Invoke,
    {
        self.instantiate()
            .context("failed to instantiate component")?
            .call::<C>(instance, name, incoming, outgoing)
            .await
    }
}

impl From<Component> for Option<jwt::Claims<jwt::Component>> {
    fn from(Component { claims, .. }: Component) -> Self {
        claims
    }
}

/// An instance of a [Component]
pub struct Instance {
    store: wasmtime::Store<Ctx>,
    instance_pre: InstancePre<Ctx>,
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

    /// Set component stderr stream. If another stderr was set, it is replaced and the old one is flushed and shut down.
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

    /// Invoke an operation on an [Instance] producing a result.
    #[instrument(skip(self, instance, name, incoming, outgoing), fields(interface = instance, function = name))]
    pub async fn call<C>(
        &mut self,
        instance: &str,
        name: &str,
        incoming: C::Incoming,
        outgoing: C::Outgoing,
    ) -> anyhow::Result<()>
    where
        C: wrpc_transport::Invoke,
    {
        let component = self
            .instance_pre
            .instantiate_async(&mut self.store.as_context_mut())
            .await
            .context("failed to instantiate component")?;

        let func = {
            let mut exports = component.exports(&mut self.store);
            if instance.is_empty() {
                exports.root()
            } else {
                exports
                    .instance(instance)
                    .with_context(|| format!("instance of `{instance}` not found"))?
            }
            .func(name)
            .with_context(|| format!("function `{name}` not found"))?
        };

        let results_ty = func.results(&self.store);
        let mut results = vec![Val::Bool(false); results_ty.len()];

        let params = func.params(&self.store);
        let mut params_values = vec![Val::Bool(false); params.len()];

        // Decode params
        let mut incoming = pin!(incoming);
        for (i, (v, ty)) in zip(params_values.iter_mut(), &*params).enumerate() {
            read_value(&mut self.store, &mut incoming, v, ty, &[i])
                .await
                .context("failed to decode result value")?;
        }

        func.call_async(&mut self.store, &params_values, &mut results)
            .await
            .context("failed to call function")?;
        func.post_return_async(&mut self.store)
            .await
            .context("failed to perform post-return cleanup")?;

        // Stream the results back
        // NOTE: All results will be provided synchronously from wasm calls
        let mut buf = BytesMut::default();
        let mut deferred = vec![];
        for (v, ty) in zip(results.iter_mut(), &*func.results(&mut self.store)) {
            let mut enc: ValEncoder<Ctx, <C as Invoke>::Outgoing> =
                ValEncoder::new(self.store.as_context_mut(), ty);
            enc.encode(v, &mut buf).context("failed to encode result")?;
            deferred.push(enc.deferred);
        }

        let mut outgoing = pin!(outgoing);

        outgoing
            .as_mut()
            .write_all(&buf)
            .await
            .context("failed to write results to outgoing stream")?;
        outgoing
            .as_mut()
            .shutdown()
            .await
            .context("failed to shutdown outgoing stream")?;

        Ok(())
    }
}

/// Instance of a guest interface `T`
pub struct InterfaceInstance<T> {
    store: Mutex<wasmtime::Store<Ctx>>,
    bindings: T,
}

#[inline]
async fn read_flags(n: usize, r: &mut (impl AsyncRead + Unpin)) -> std::io::Result<u128> {
    let mut buf = 0u128.to_le_bytes();
    r.read_exact(&mut buf[..n]).await?;
    Ok(u128::from_le_bytes(buf))
}

/// Read encoded value of type [`Type`] from an [`AsyncRead`] into a [`Val`]
#[instrument(level = "trace", skip_all, fields(ty, path))]
async fn read_value<T, R>(
    store: &mut impl AsContextMut<Data = T>,
    r: &mut Pin<&mut R>,
    val: &mut Val,
    ty: &Type,
    path: &[usize],
) -> std::io::Result<()>
where
    T: WasiView,
    R: AsyncRead + wrpc_transport::Index<R> + Send + Unpin + 'static,
{
    match ty {
        Type::Bool => {
            let v = r.read_bool().await?;
            *val = Val::Bool(v);
            Ok(())
        }
        Type::S8 => {
            let v = r.read_i8().await?;
            *val = Val::S8(v);
            Ok(())
        }
        Type::U8 => {
            let v = r.read_u8().await?;
            *val = Val::U8(v);
            Ok(())
        }
        Type::S16 => {
            let v = r.read_i16_leb128().await?;
            *val = Val::S16(v);
            Ok(())
        }
        Type::U16 => {
            let v = r.read_u16_leb128().await?;
            *val = Val::U16(v);
            Ok(())
        }
        Type::S32 => {
            let v = r.read_i32_leb128().await?;
            *val = Val::S32(v);
            Ok(())
        }
        Type::U32 => {
            let v = r.read_u32_leb128().await?;
            *val = Val::U32(v);
            Ok(())
        }
        Type::S64 => {
            let v = r.read_i64_leb128().await?;
            *val = Val::S64(v);
            Ok(())
        }
        Type::U64 => {
            let v = r.read_u64_leb128().await?;
            *val = Val::U64(v);
            Ok(())
        }
        Type::Float32 => {
            let v = r.read_f32_le().await?;
            *val = Val::Float32(v);
            Ok(())
        }
        Type::Float64 => {
            let v = r.read_f64_le().await?;
            *val = Val::Float64(v);
            Ok(())
        }
        Type::Char => {
            let v = r.read_char_utf8().await?;
            *val = Val::Char(v);
            Ok(())
        }
        Type::String => {
            let mut s = String::default();
            r.read_core_name(&mut s).await?;
            *val = Val::String(s);
            Ok(())
        }
        Type::List(ty) => {
            let n = r.read_u32_leb128().await?;
            let n = n.try_into().unwrap_or(usize::MAX);
            let mut vs = Vec::with_capacity(n);
            let ty = ty.ty();
            let mut path = path.to_vec();
            for i in 0..n {
                let mut v = Val::Bool(false);
                path.push(i);
                trace!(i, "reading list element value");
                Box::pin(read_value(store, r, &mut v, &ty, &path)).await?;
                path.pop();
                vs.push(v);
            }
            *val = Val::List(vs);
            Ok(())
        }
        Type::Record(ty) => {
            let fields = ty.fields();
            let mut vs = Vec::with_capacity(fields.len());
            let mut path = path.to_vec();
            for (i, Field { name, ty }) in fields.enumerate() {
                let mut v = Val::Bool(false);
                path.push(i);
                trace!(i, "reading struct field value");
                Box::pin(read_value(store, r, &mut v, &ty, &path)).await?;
                path.pop();
                vs.push((name.to_string(), v));
            }
            *val = Val::Record(vs);
            Ok(())
        }
        Type::Tuple(ty) => {
            let types = ty.types();
            let mut vs = Vec::with_capacity(types.len());
            let mut path = path.to_vec();
            for (i, ty) in types.enumerate() {
                let mut v = Val::Bool(false);
                path.push(i);
                trace!(i, "reading tuple element value");
                Box::pin(read_value(store, r, &mut v, &ty, &path)).await?;
                path.pop();
                vs.push(v);
            }
            *val = Val::Tuple(vs);
            Ok(())
        }
        Type::Variant(ty) => {
            let discriminant = r.read_u32_leb128().await?;
            let discriminant = discriminant
                .try_into()
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            let Case { name, ty } = ty.cases().nth(discriminant).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown variant discriminant `{discriminant}`"),
                )
            })?;
            let name = name.to_string();
            if let Some(ty) = ty {
                let mut v = Val::Bool(false);
                trace!(variant = name, "reading nested variant value");
                Box::pin(read_value(store, r, &mut v, &ty, path)).await?;
                *val = Val::Variant(name, Some(Box::new(v)));
            } else {
                *val = Val::Variant(name, None);
            }
            Ok(())
        }
        Type::Enum(ty) => {
            let discriminant = r.read_u32_leb128().await?;
            let discriminant = discriminant
                .try_into()
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
            let name = ty.names().nth(discriminant).ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown enum discriminant `{discriminant}`"),
                )
            })?;
            *val = Val::Enum(name.to_string());
            Ok(())
        }
        Type::Option(ty) => {
            let ok = r.read_option_status().await?;
            if ok {
                let mut v = Val::Bool(false);
                trace!("reading nested `option::some` value");
                Box::pin(read_value(store, r, &mut v, &ty.ty(), path)).await?;
                *val = Val::Option(Some(Box::new(v)));
            } else {
                *val = Val::Option(None);
            }
            Ok(())
        }
        Type::Result(ty) => {
            let ok = r.read_result_status().await?;
            if ok {
                if let Some(ty) = ty.ok() {
                    let mut v = Val::Bool(false);
                    trace!("reading nested `result::ok` value");
                    Box::pin(read_value(store, r, &mut v, &ty, path)).await?;
                    *val = Val::Result(Ok(Some(Box::new(v))));
                } else {
                    *val = Val::Result(Ok(None));
                }
            } else if let Some(ty) = ty.err() {
                let mut v = Val::Bool(false);
                trace!("reading nested `result::err` value");
                Box::pin(read_value(store, r, &mut v, &ty, path)).await?;
                *val = Val::Result(Err(Some(Box::new(v))));
            } else {
                *val = Val::Result(Err(None));
            }
            Ok(())
        }
        Type::Flags(ty) => {
            let names = ty.names();
            let flags = match names.len() {
                ..=8 => read_flags(1, r).await?,
                9..=16 => read_flags(2, r).await?,
                17..=24 => read_flags(3, r).await?,
                25..=32 => read_flags(4, r).await?,
                33..=40 => read_flags(5, r).await?,
                41..=48 => read_flags(6, r).await?,
                49..=56 => read_flags(7, r).await?,
                57..=64 => read_flags(8, r).await?,
                65..=72 => read_flags(9, r).await?,
                73..=80 => read_flags(10, r).await?,
                81..=88 => read_flags(11, r).await?,
                89..=96 => read_flags(12, r).await?,
                97..=104 => read_flags(13, r).await?,
                105..=112 => read_flags(14, r).await?,
                113..=120 => read_flags(15, r).await?,
                121..=128 => r.read_u128_le().await?,
                bits @ 129.. => {
                    let mut cap = bits / 8;
                    if bits % 8 != 0 {
                        cap = cap.saturating_add(1);
                    }
                    let mut buf = vec![0; cap];
                    r.read_exact(&mut buf).await?;
                    let mut vs = Vec::with_capacity(
                        buf.iter()
                            .map(|b| b.count_ones())
                            .sum::<u32>()
                            .try_into()
                            .unwrap_or(usize::MAX),
                    );
                    for (i, name) in names.enumerate() {
                        if buf[i / 8] & (1 << (i % 8)) != 0 {
                            vs.push(name.to_string());
                        }
                    }
                    *val = Val::Flags(vs);
                    return Ok(());
                }
            };
            let mut vs = Vec::with_capacity(flags.count_ones().try_into().unwrap_or(usize::MAX));
            for (i, name) in zip(0.., names) {
                if flags & (1 << i) != 0 {
                    vs.push(name.to_string());
                }
            }
            *val = Val::Flags(vs);
            Ok(())
        }
        Type::Own(ty) | Type::Borrow(ty) => {
            if *ty == ResourceType::host::<InputStream>() {
                let mut store = store.as_context_mut();
                let r = r
                    .index(path)
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
                // TODO: Implement a custom reader, this approach ignores the stream end (`\0`),
                // which will could potentially break/hang with some transports
                let res = store
                    .data_mut()
                    .table()
                    .push(InputStream::Host(Box::new(AsyncReadStream::new(
                        FramedRead::new(r, ListDecoderU8::default())
                            .into_async_read()
                            .compat(),
                    ))))
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::OutOfMemory, err))?;
                let v = res
                    .try_into_resource_any(store)
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
                *val = Val::Resource(v);
                Ok(())
            } else {
                let mut store = store.as_context_mut();
                let n = r.read_u32_leb128().await?;
                let n = usize::try_from(n)
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
                let mut buf = Vec::with_capacity(n);
                r.read_to_end(&mut buf).await?;
                let table = store.data_mut().table();
                let resource = table
                    .push(RemoteResource(buf.into()))
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::OutOfMemory, err))?;
                let resource = resource
                    .try_into_resource_any(store)
                    .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
                *val = Val::Resource(resource);
                Ok(())
            }
        }
    }
}
