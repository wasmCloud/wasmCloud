use crate::capability::builtin::{LatticeInterfaceTarget, TargetEntity};
use crate::capability::{builtin, Interfaces};
use crate::component::claims;
use crate::wrpc::{from_wrpc_value, to_wrpc_value};
use crate::Runtime;

use core::fmt::{self, Debug};
use core::iter::zip;
use core::ops::{Deref, DerefMut};
use core::pin::pin;
use core::time::Duration;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::executor::block_on;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, RwLock};
use tokio_util::codec::Encoder;
use tracing::{debug, error, instrument, trace, warn};
use wascap::jwt;
use wasmcloud_component_adapters::WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER;
use wasmcloud_core::{CallTargetInterface, LatticeTarget};
use wasmcloud_tracing::context::TraceContextInjector;
use wasmtime::component::{
    self, types, InstancePre, Linker, ResourceTable, ResourceTableError, Val,
};
use wasmtime::AsContextMut;
use wasmtime_wasi::pipe::{AsyncWriteStream, ClosedInputStream, ClosedOutputStream};
use wasmtime_wasi::{
    HostInputStream, HostOutputStream, StdinStream, StdoutStream, StreamError, StreamResult,
    Subscribe, WasiCtx, WasiCtxBuilder, WasiView,
};
use wasmtime_wasi_http::WasiHttpCtx;
use wit_parser::{Function, Resolve, WorldItem, WorldKey};
use wrpc_runtime_wasmtime::{read_value, ValEncoder, WrpcView};
use wrpc_transport::Invoke;
use wrpc_transport_nats::{Client, Reader, SubjectWriter};

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

/// Context used for running executions
pub struct Ctx {
    client: Arc<wrpc_transport_nats::Client>,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    handler: builtin::Handler,
    stderr: StdioStream<Box<dyn HostOutputStream>>,
}

impl WrpcView<wrpc_transport_nats::Client> for Ctx {
    fn client(&self) -> &wrpc_transport_nats::Client {
        &self.client
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
    exports: Arc<HashMap<String, HashMap<String, Function>>>,
    ty: types::Component,
    instance_pre: wasmtime::component::InstancePre<Ctx>,
    max_execution_time: Duration,
    component_id: String,
    client: Arc<async_nats::Client>,
    // A map of function names to their respective paths for wrpc params
    paths: Arc<HashMap<String, Arc<[Arc<[Option<usize>]>]>>>,
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
            .field("paths", &self.paths)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

/// Polyfills all missing imports and returns instance -> function -> type map for each polyfill
#[instrument(level = "trace", skip_all)]
async fn polyfill<'a, T>(
    resolve: &wit_parser::Resolve,
    imports: T,
    engine: &wasmtime::Engine,
    comp_ty: &types::Component,
    linker: &mut Linker<Ctx>,
    component_id: &str,
    interface_links: &Arc<RwLock<HashMap<String, HashMap<String, HashMap<String, LatticeTarget>>>>>,
    targets: &Arc<RwLock<HashMap<CallTargetInterface, String>>>,
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
            comp_ty.get_import(engine, &instance_name)
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

        let LatticeInterfaceTarget { link_name, .. } =
            match identify_wrpc_target(interface_links, targets, &target, component_id).await {
                Some(target) => target,
                None => {
                    error!(?instance_name, "failed to identify wrpc target");
                    continue;
                }
            };
        let injector = TraceContextInjector::default_with_span();
        let mut headers = injector_to_headers(&injector);
        headers.insert("source-id", component_id.as_ref());
        headers.insert("link-name", link_name.as_str());

        if let Err(err) = wrpc_runtime_wasmtime::link_instance(
            engine,
            &mut linker,
            instance,
            instance_name.as_ref(),
            Some(headers),
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
    client: Arc<async_nats::Client>,
    lattice: &str,
    component_id: &str,
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

    let client = wrpc_transport_nats::Client::new(
        Arc::clone(&client),
        format!("{}.{}", lattice, component_id),
    );

    let handler = handler.into();
    let ctx = Ctx {
        client: Arc::new(client),
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
    pub async fn new(
        rt: &Runtime,
        wasm: impl AsRef<[u8]>,
        component_id: String,
        lattice: &str,
        interface_links: &Arc<
            RwLock<HashMap<String, HashMap<String, HashMap<String, LatticeTarget>>>>,
        >,
        targets: &Arc<RwLock<HashMap<CallTargetInterface, String>>>,
        client: Arc<async_nats::Client>,
    ) -> anyhow::Result<Self> {
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
            return Box::pin(Self::new(
                rt,
                wasm,
                component_id,
                lattice,
                interface_links,
                targets,
                client,
            ))
            .await;
        }
        let engine = rt.engine.clone();
        let claims = claims(wasm)?;
        let component = wasmtime::component::Component::new(&engine, wasm)
            .context("failed to compile component")?;

        let mut linker = Linker::new(&engine);

        Interfaces::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasmcloud:host/interfaces` interface")?;

        wasmtime_wasi::add_to_linker_async(&mut linker)
            .context("failed to link core WASI interfaces")?;

        wasmtime_wasi_http::proxy::add_to_linker(&mut linker)
            .context("failed to link `wasi:http/proxy` interface")?;
        wasmtime_wasi_http::proxy::sync::add_to_linker(&mut linker)
            .context("failed to link `wasi:http/proxy` sync interface")?;

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
        polyfill(
            &resolve,
            imports,
            &engine,
            &ty,
            &mut linker,
            &component_id,
            interface_links,
            targets,
        )
        .await;
        let instance_pre = linker.instantiate_pre(&component)?;
        // TODO: Record the substituted type exports, not parser exports

        // Gather paths for wrpc transport for each exported function
        let mut paths: HashMap<String, Arc<[Arc<[Option<usize>]>]>> = HashMap::new();
        for (_, world_item) in exports {
            if let WorldItem::Function(func) = world_item {
                let func_paths: Arc<[Arc<[Option<usize>]>]> = func
                    .params
                    .iter()
                    .map(|(_, ty)| {
                        let (nested, _is_fut) = wrpc_introspect::async_paths_ty(&resolve, ty);
                        nested
                            .into_iter()
                            .flat_map(|path| path.into_iter().map(|x| x.map(|y| y as usize)))
                            .collect()
                    })
                    .collect();
                paths.insert(func.name.clone(), func_paths);
            }
            if let WorldItem::Interface(id) = world_item {
                for func in resolve.interfaces[*id].functions.values() {
                    let func_paths: Arc<[Arc<[Option<usize>]>]> = func
                        .params
                        .iter()
                        .map(|(_, ty)| {
                            let (nested, _is_fut) = wrpc_introspect::async_paths_ty(&resolve, ty);
                            nested
                                .into_iter()
                                .flat_map(|path| path.into_iter().map(|x| x.map(|y| y as usize)))
                                .collect()
                        })
                        .collect();
                    paths.insert(func.name.clone(), func_paths);
                }
            }
        }

        Ok(Self {
            engine,
            claims,
            handler: rt.handler.clone(),
            exports: Arc::new(function_exports(&resolve, exports)),
            ty,
            instance_pre,
            max_execution_time: rt.max_execution_time,
            client,
            paths: Arc::new(paths),
            component_id,
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
    pub async fn read(
        rt: &Runtime,
        mut wasm: impl AsyncRead + Unpin,
        component_id: &str,
        lattice: &str,
        client: Arc<async_nats::Client>,
        interface_links: &Arc<
            RwLock<HashMap<String, HashMap<String, HashMap<String, LatticeTarget>>>>,
        >,
        targets: &Arc<RwLock<HashMap<CallTargetInterface, String>>>,
    ) -> anyhow::Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf)
            .await
            .context("failed to read Wasm")?;
        Self::new(
            rt,
            buf,
            component_id.to_string(),
            lattice,
            interface_links,
            targets,
            client,
        )
        .await
    }

    /// Reads the WebAssembly binary synchronously and calls [Component::new].
    ///
    /// # Errors
    ///
    /// Fails if either reading `wasm` fails or [Self::new] fails
    #[instrument(skip(wasm))]
    pub fn read_sync(
        rt: &Runtime,
        mut wasm: impl std::io::Read,
        component_id: &str,
        lattice: &str,
        client: Arc<async_nats::Client>,
        interface_links: &Arc<
            RwLock<HashMap<String, HashMap<String, HashMap<String, LatticeTarget>>>>,
        >,
        targets: &Arc<RwLock<HashMap<CallTargetInterface, String>>>,
    ) -> anyhow::Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf).context("failed to read Wasm")?;
        block_on(Self::new(
            rt,
            buf,
            component_id.to_string(),
            lattice,
            interface_links,
            targets,
            client,
        ))
    }

    /// Returns a map of dynamic function export types.
    /// Top level map is keyed by the instance name.
    /// Inner map is keyed by exported function name.
    #[must_use]
    pub fn exports(&self) -> &Arc<HashMap<String, HashMap<String, Function>>> {
        &self.exports
    }

    /// Returns the paths required for wrpc transport for a specific function name.
    pub fn paths(&self) -> &Arc<HashMap<String, Arc<[Arc<[Option<usize>]>]>>> {
        &self.paths
    }

    /// [Claims](jwt::Claims) associated with this [Component].
    #[instrument(level = "trace")]
    pub fn claims(&self) -> Option<&jwt::Claims<jwt::Component>> {
        self.claims.as_ref()
    }

    /// Like [Self::instantiate], but moves the [Component].
    #[instrument]
    pub fn into_instance(self, lattice: &str) -> anyhow::Result<Instance> {
        self.instantiate(lattice)
    }

    /// Like [Self::instantiate], but moves the [Component] and returns the associated [jwt::Claims].
    #[instrument]
    pub fn into_instance_claims(
        self,
        lattice: &str,
    ) -> anyhow::Result<(Instance, Option<jwt::Claims<jwt::Component>>)> {
        let instance = instantiate(
            &self.engine,
            self.handler,
            self.ty,
            self.instance_pre,
            self.max_execution_time,
            self.client,
            lattice,
            &self.component_id,
        )?;
        Ok((instance, self.claims))
    }

    /// Instantiates a [Component] for the specified lattice
    /// and returns the resulting [Instance].
    #[instrument(level = "debug", skip(self))]
    pub fn instantiate(&self, lattice: &str) -> anyhow::Result<Instance> {
        instantiate(
            &self.engine,
            self.handler.clone(),
            self.ty.clone(),
            self.instance_pre.clone(),
            self.max_execution_time,
            Arc::clone(&self.client),
            lattice,
            &self.component_id,
        )
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
        incoming: Reader,
        outgoing: SubjectWriter,
    ) -> anyhow::Result<()> {
        let component = self
            .instance_pre
            .instantiate_async(&mut self.store)
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
            let mut enc: ValEncoder<Ctx, <Client as Invoke>::Outgoing> =
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

    /// Invoke an operation on an [Instance] producing a result, using legacy api.
    #[instrument(level = "debug", skip(self, params, instance, name), fields(interface = instance, function = name))]
    pub async fn call_legacy(
        &mut self,
        instance: &str,
        name: &str,
        params: Vec<wrpc_transport_legacy::Value>,
    ) -> anyhow::Result<Vec<wrpc_transport_legacy::Value>> {
        let component = self
            .instance_pre
            .instantiate_async(&mut self.store)
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
        let params: Vec<_> = zip(params, func.params(&self.store).iter())
            .map(|(val, ty)| from_wrpc_value(&mut self.store, val, ty))
            .collect::<anyhow::Result<_>>()
            .context("failed to convert wasmtime values to wRPC values")?;
        let results_ty = func.results(&self.store);
        let mut results = vec![Val::Bool(false); results_ty.len()];
        func.call_async(&mut self.store, &params, &mut results)
            .await
            .context("failed to call function")?;
        func.post_return_async(&mut self.store)
            .await
            .context("failed to perform post-return cleanup")?;
        zip(results, results_ty.iter())
            .map(|(val, ty)| to_wrpc_value(&mut self.store, &val, ty))
            .collect::<anyhow::Result<_>>()
            .context("failed to convert wasmtime values to wRPC values")
    }
}

/// Instance of a guest interface `T`
pub struct InterfaceInstance<T> {
    store: Mutex<wasmtime::Store<Ctx>>,
    bindings: T,
}

pub fn function_exports(
    resolve: &Resolve,
    exports: &IndexMap<WorldKey, WorldItem>,
) -> HashMap<String, HashMap<String, Function>> {
    let mut result = HashMap::new();

    for (key, item) in exports {
        if let WorldItem::Interface(interface_id) = item {
            if let Some(interface) = resolve.interfaces.get(*interface_id) {
                let mut functions = HashMap::new();
                for (func_name, func) in &interface.functions {
                    functions.insert(func_name.clone(), func.clone());
                }
                result.insert(key.clone().into(), functions);
            }
        }
    }

    result
}

fn injector_to_headers(injector: &TraceContextInjector) -> async_nats::header::HeaderMap {
    injector
        .iter()
        .filter_map(|(k, v)| {
            // There's not really anything we can do about headers that don't parse
            let name = async_nats::header::HeaderName::from_str(k.as_str()).ok()?;
            let value = async_nats::header::HeaderValue::from_str(v.as_str()).ok()?;
            Some((name, value))
        })
        .collect()
}

#[instrument(level = "trace")]
async fn identify_interface_target(
    interface_links: &Arc<RwLock<HashMap<String, HashMap<String, HashMap<String, LatticeTarget>>>>>,
    targets: &Arc<RwLock<HashMap<CallTargetInterface, String>>>,
    target_interface: &CallTargetInterface,
    component_id: &str,
) -> Option<TargetEntity> {
    let links = interface_links.read().await;
    let targets = targets.read().await;
    let link_name = targets
        .get(target_interface)
        .map_or("default", String::as_str);
    let (namespace, package, interface) = target_interface.as_parts();

    // Determine the lattice target ID we should be sending to
    let lattice_target_id = links
        .get(link_name)
        .and_then(|packages| packages.get(&format!("{namespace}:{package}")))
        .and_then(|interfaces| interfaces.get(interface));

    // If we managed to find a target ID, convert it into an entity
    let target_entity = lattice_target_id.map(|id| {
        TargetEntity::Lattice(LatticeInterfaceTarget {
            id: id.clone(),
            interface: target_interface.clone(),
            link_name: link_name.to_string(),
        })
    });

    if target_entity.is_none() {
        debug!(
            ?links,
            interface,
            namespace,
            package,
            component_id,
            "component is not linked to a lattice target for the given interface"
        );
    }
    target_entity
}

async fn identify_wrpc_target(
    interface_links: &Arc<RwLock<HashMap<String, HashMap<String, HashMap<String, LatticeTarget>>>>>,
    targets: &Arc<RwLock<HashMap<CallTargetInterface, String>>>,
    target_interface: &CallTargetInterface,
    component_id: &str,
) -> Option<LatticeInterfaceTarget> {
    let target =
        identify_interface_target(interface_links, targets, target_interface, component_id).await;
    let Some(TargetEntity::Lattice(lattice_target)) = target else {
        return None;
    };
    Some(lattice_target)
}
