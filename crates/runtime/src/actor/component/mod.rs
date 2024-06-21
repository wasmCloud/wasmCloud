use crate::actor::claims;
use crate::capability::{builtin, Bus, Interfaces};
use crate::Runtime;

use core::fmt::{self, Debug};
use core::iter::zip;
use core::ops::{Deref, DerefMut};
use core::pin::pin;
use core::time::Duration;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::future::try_join_all;
use futures::try_join;
use indexmap::IndexMap;
use std::collections::{hash_map, HashMap};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite};
use tokio::sync::Mutex;
use tokio_util::codec::Encoder;
use tracing::{error, instrument, trace, warn};
use wascap::jwt;
use wasmcloud_component_adapters::WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER;
use wasmcloud_core::CallTargetInterface;
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
use wrpc_runtime_wasmtime::read_value;
use wrpc_runtime_wasmtime::ValEncoder;
use wrpc_transport::Index;
use wrpc_transport::Session;
use wrpc_transport::{Invocation, Invoke};
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
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    handler: builtin::Handler,
    stderr: StdioStream<Box<dyn HostOutputStream>>,
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
    polyfills: Arc<HashMap<String, HashMap<String, Function>>>,
    exports: Arc<HashMap<String, HashMap<String, Function>>>,
    ty: types::Component,
    instance_pre: wasmtime::component::InstancePre<Ctx>,
    max_execution_time: Duration,
    // A map of function names to their respective paths for params
    paths: Arc<HashMap<String, Vec<Vec<Option<usize>>>>>,
}

impl Debug for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO - add new fields
        f.debug_struct("Component")
            .field("claims", &self.claims)
            .field("handler", &self.handler)
            .field("runtime", &"wasmtime")
            .field("polyfills", &self.polyfills)
            .field("exports", &self.exports)
            .field("ty", &self.ty)
            .field("max_execution_time", &self.max_execution_time)
            .field("paths", &self.paths)
            .finish_non_exhaustive()
    }
}

/// Polyfills all missing imports and returns instance -> function -> type map for each polyfill
#[instrument(level = "trace", skip_all)]
fn polyfill<'a, T>(
    resolve: &wit_parser::Resolve,
    imports: T,
    engine: &wasmtime::Engine,
    comp_ty: &types::Component,
    linker: &mut Linker<Ctx>,
) -> HashMap<String, HashMap<String, Function>>
where
    T: IntoIterator<Item = (&'a wit_parser::WorldKey, &'a wit_parser::WorldItem)>,
    T::IntoIter: ExactSizeIterator,
{
    let imports = imports.into_iter();
    let mut polyfills = HashMap::with_capacity(imports.len());
    for (wk, item) in imports {
        let instance_name = resolve.name_world_key(wk);
        // Avoid polyfilling instances, for which static bindings are linked
        skip_static_instances!(instance_name.as_ref());
        let wit_parser::WorldItem::Interface(interface) = item else {
            continue;
        };
        let Some(wit_parser::Interface {
            name: interface_name,
            functions,
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
        let target = Arc::new(CallTargetInterface {
            namespace: package_name.namespace.to_string(),
            package: package_name.name.to_string(),
            interface: interface_name.to_string(),
        });
        let Some(types::ComponentItem::ComponentInstance(_instance)) =
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
        let hash_map::Entry::Vacant(instance_import) = polyfills.entry(instance_name.to_string())
        else {
            error!("duplicate instance import");
            continue;
        };
        let mut function_imports = HashMap::with_capacity(functions.len());
        let instance_name = Arc::new(instance_name);
        for (func_name, ty) in functions {
            trace!(
                ?instance_name,
                func_name,
                "polyfill component function import"
            );

            let hash_map::Entry::Vacant(func_import) =
                function_imports.entry(func_name.to_string())
            else {
                error!("duplicate function import");
                continue;
            };

            let instance_name = Arc::clone(&instance_name);
            let func_name = Arc::new(func_name.to_string());
            let target = Arc::clone(&target);
            let rpc_name = wrpc_introspect::rpc_func_name(ty);
            let (param_types, result_types) = match comp_ty.get_import(engine, &ty.name) {
                Some(types::ComponentItem::ComponentFunc(i)) => (
                    i.params().collect::<Vec<_>>().into_iter(),
                    i.results().collect::<Vec<_>>().into_iter(),
                ),
                Some(_) | None => {
                    error!(
                        "Function param types not found for instance: {} and name: {}",
                        instance_name, func_name
                    );
                    continue;
                }
            };

            let result_types_vec: Vec<_> = result_types.collect();
            let result_types_arc = Arc::new(result_types_vec);

            if let Err(err) = linker.func_new_async(rpc_name, move |mut store, params, results| {
                let instance_name = Arc::clone(&instance_name);
                let func_name = Arc::clone(&func_name);
                let target = Arc::clone(&target);
                let result_types_arc = Arc::clone(&result_types_arc);

                let handler = store.data().handler.clone();

                let func_paths = match handler.get_func_paths(&instance_name, &func_name) {
                    Some(func_paths) => func_paths,
                    None => {
                        error!(
                            "Function paths not found for instance: {} and name: {}",
                            instance_name, func_name
                        );
                        return Box::new(async { Ok::<(), anyhow::Error>(()) });
                    }
                };

                // Encode sync params and gather deferred asyncs
                let mut buf = BytesMut::default();
                let mut deferred = vec![];
                for (v, ref ty) in zip(&*params, param_types.clone()) {
                    let mut enc: ValEncoder<Ctx, <Client as Invoke>::Outgoing> =
                        ValEncoder::new(store.as_context_mut(), ty);
                    if let Err(err) = enc
                        .encode(v, &mut buf)
                        .context("failed to encode parameter")
                    {
                        error!(?err, "failed to encode parameter");
                        return Box::new(async { Ok::<(), anyhow::Error>(()) });
                    }
                    deferred.push(enc.deferred);
                }

                Box::new(async move {
                    let target = match handler.identify_interface_target(&target).await {
                        Some(target) => target,
                        None => {
                            error!("failed to identify interface target");
                            return Ok::<(), anyhow::Error>(());
                        }
                    };

                    let Invocation {
                        outgoing,
                        incoming,
                        session,
                    } = handler
                        .call(
                            target,
                            &instance_name,
                            &func_name,
                            buf,
                            func_paths
                                .iter()
                                .map(|v| v.iter().map(|a| a.as_ref().map(|&x| x)).collect())
                                .collect(),
                        )
                        .await
                        .context("failed to call target interface")?;

                    let results_incoming = try_join!(
                        // Stream async params
                        async {
                            try_join_all(
                                zip(0.., deferred)
                                    .filter_map(|(i, f)| f.map(|f| (outgoing.index(&[i]), f)))
                                    .map(|(w, f)| async move {
                                        let w = w.map_err(Into::<anyhow::Error>::into)?;
                                        f(w).await
                                    }),
                            )
                            .await
                            .context("failed to write asynchronous parameters")?;
                            pin!(outgoing)
                                .shutdown()
                                .await
                                .context("failed to shutdown outgoing stream")
                        },
                        // Receive returns
                        async {
                            let mut incoming = pin!(incoming);
                            let mut results_incoming = Vec::with_capacity(result_types_arc.len());
                            for (i, ty) in result_types_arc.iter().enumerate() {
                                let mut val = Val::Bool(false);
                                read_value(
                                    &mut store.as_context_mut(),
                                    &mut incoming,
                                    &mut val,
                                    &ty,
                                    &[i],
                                )
                                .await
                                .context("failed to decode result value")?;
                                results_incoming.push(val);
                            }
                            Ok(results_incoming)
                        },
                    )?
                    .1;

                    session
                        .finish(Ok(()))
                        .await
                        .map_err(|err| anyhow!(err).context("session failed"))?;

                    // Ensure the result values are properly set
                    for (result, value) in results.iter_mut().zip(results_incoming) {
                        *result = value;
                    }
                    Ok::<(), anyhow::Error>(())
                })
            }) {
                error!(?err, "failed to polyfill component function import");
            }
            func_import.insert(ty.clone());
        }
        instance_import.insert(function_imports);
    }
    polyfills
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
        let polyfills = Arc::new(polyfill(&resolve, imports, &engine, &ty, &mut linker));
        let instance_pre = linker.instantiate_pre(&component)?;
        // TODO: Record the substituted type exports, not parser exports

        // Gather paths for wrpc transport for each exported function
        let mut paths: HashMap<String, Vec<Vec<Option<usize>>>> = HashMap::new();
        for (_, world_item) in exports {
            if let WorldItem::Function(func) = world_item {
                let mut func_paths = Vec::new();
                for (_, ty) in func.params.iter() {
                    let (nested, _is_fut) = wrpc_introspect::async_paths_ty(&resolve, ty);
                    for path in nested {
                        func_paths.push(
                            path.into_iter()
                                .map(|opt| opt.map(|x| x as usize))
                                .collect(),
                        );
                    }
                }
                paths.insert(func.name.clone(), func_paths);
            }
            if let WorldItem::Interface(id) = world_item {
                for func in resolve.interfaces[*id].functions.values() {
                    let mut func_paths = Vec::new();
                    for (_, ty) in func.params.iter() {
                        let (nested, _is_fut) = wrpc_introspect::async_paths_ty(&resolve, ty);
                        for path in nested {
                            func_paths.push(
                                path.into_iter()
                                    .map(|opt| opt.map(|x| x as usize))
                                    .collect(),
                            );
                        }
                    }
                    paths.insert(func.name.clone(), func_paths);
                }
            }
        }

        Ok(Self {
            engine,
            claims,
            handler: rt.handler.clone(),
            polyfills,
            exports: Arc::new(function_exports(&resolve, exports)),
            ty,
            instance_pre,
            max_execution_time: rt.max_execution_time,
            paths: Arc::new(paths),
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
    pub fn exports(&self) -> &Arc<HashMap<String, HashMap<String, Function>>> {
        &self.exports
    }

    /// Returns the paths required for wrpc transport for a specific function name.
    pub fn paths(&self) -> &Arc<HashMap<String, Vec<Vec<Option<usize>>>>> {
        &self.paths
    }

    /// Returns a map of dynamic polyfilled function import types.
    /// Top level map is keyed by the instance name.
    /// Inner map is keyed by exported function name.
    #[must_use]
    pub fn polyfills(&self) -> &Arc<HashMap<String, HashMap<String, Function>>> {
        &self.polyfills
    }

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
