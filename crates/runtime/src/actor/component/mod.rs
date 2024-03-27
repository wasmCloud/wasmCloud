use crate::actor::claims;
use crate::capability::{builtin, Bus, Interfaces};
use crate::Runtime;

use core::fmt::{self, Debug};
use core::iter::zip;
use core::ops::{Deref, DerefMut};

use std::collections::{hash_map, HashMap};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_trait::async_trait;
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite};
use tokio::sync::Mutex;
use tracing::{error, instrument, trace, warn};
use wascap::jwt;
use wasmcloud_component_adapters::WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER;
use wasmcloud_core::CallTargetInterface;
use wasmtime::component::{
    self, types, InstancePre, Linker, ResourceTable, ResourceTableError, Type, Val,
};
use wasmtime_wasi::command;
use wasmtime_wasi::pipe::{AsyncWriteStream, ClosedInputStream, ClosedOutputStream};
use wasmtime_wasi::{
    HostInputStream, HostOutputStream, StdinStream, StdoutStream, StreamError, StreamResult,
    Subscribe, WasiCtx, WasiCtxBuilder, WasiView,
};
use wasmtime_wasi_http::WasiHttpCtx;
use wrpc_runtime_wasmtime::{from_wrpc_value, to_wrpc_value};
use wrpc_types::{function_exports, DynamicFunction};

mod blobstore;
mod bus;
mod http;
mod keyvalue;
mod logging;
mod messaging;

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
    custom_result_types: HashMap<String, HashMap<String, Vec<Type>>>,
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

/// Pre-compiled actor [Component], which is cheapily-[Cloneable](Clone)
#[derive(Clone)]
pub struct Component {
    engine: wasmtime::Engine,
    claims: Option<jwt::Claims<jwt::Actor>>,
    handler: builtin::HandlerBuilder,
    polyfilled_imports: Arc<HashMap<String, HashMap<String, DynamicFunction>>>,
    exports: Arc<HashMap<String, HashMap<String, DynamicFunction>>>,
    ty: types::Component,
    instance_pre: wasmtime::component::InstancePre<Ctx>,
}

impl Debug for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Component")
            .field("claims", &self.claims)
            .field("handler", &self.handler)
            .field("runtime", &"wasmtime")
            .field("polyfilled_imports", &self.polyfilled_imports)
            .field("exports", &self.exports)
            .field("ty", &self.ty)
            .finish_non_exhaustive()
    }
}

/// Polyfills all missing imports and returns instance -> function -> type map for each polyfill
#[instrument(level = "trace", skip_all)]
fn polyfill<'a, T>(
    resolve: &wit_parser::Resolve,
    imports: T,
    component: &component::Component,
    linker: &mut Linker<Ctx>,
) -> HashMap<String, HashMap<String, DynamicFunction>>
where
    T: IntoIterator<Item = (&'a wit_parser::WorldKey, &'a wit_parser::WorldItem)>,
    T::IntoIter: ExactSizeIterator,
{
    let imports = imports.into_iter();
    let mut polyfilled_imports = HashMap::with_capacity(imports.len());
    for (wk, item) in imports {
        let instance_name = resolve.name_world_key(wk);
        match instance_name.as_ref() {
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
            | "wasi:filesystem/preopens@0.2.0"
            | "wasi:filesystem/types@0.2.0"
            | "wasi:http/incoming-handler@0.2.0"
            | "wasi:http/outgoing-handler@0.2.0"
            | "wasi:http/types@0.2.0"
            | "wasi:io/error@0.2.0"
            | "wasi:io/poll@0.2.0"
            | "wasi:io/streams@0.2.0"
            | "wasi:keyvalue/atomic@0.2.0-draft"
            | "wasi:keyvalue/eventual@0.2.0-draft"
            | "wasi:keyvalue/types@0.2.0-draft"
            | "wasi:keyvalue/wasi-keyvalue-error@0.2.0-draft"
            | "wasi:logging/logging"
            | "wasi:random/random@0.2.0"
            | "wasi:sockets/instance-network@0.2.0"
            | "wasi:sockets/network@0.2.0"
            | "wasi:sockets/tcp-create-socket@0.2.0"
            | "wasi:sockets/tcp@0.2.0"
            | "wasi:sockets/udp-create-socket@0.2.0"
            | "wasi:sockets/udp@0.2.0"
            | "wasmcloud:bus/guest-config"
            | "wasmcloud:bus/lattice"
            | "wasmcloud:messaging/consumer"
            | "wasmcloud:messaging/handler"
            | "wasmcloud:messaging/types" => continue,
            _ => {}
        }
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
        let hash_map::Entry::Vacant(instance_import) =
            polyfilled_imports.entry(instance_name.to_string())
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
            let ty = match DynamicFunction::resolve(resolve, ty) {
                Ok(ty) => ty,
                Err(err) => {
                    error!(?err, "failed to resolve polyfilled function type");
                    continue;
                }
            };
            let hash_map::Entry::Vacant(func_import) =
                function_imports.entry(func_name.to_string())
            else {
                error!("duplicate function import");
                continue;
            };
            let instance_name = Arc::clone(&instance_name);
            let func_name = Arc::new(func_name.to_string());
            let target = Arc::clone(&target);
            if let Err(err) = linker.func_new_async(
                component,
                Arc::clone(&func_name).as_str(),
                move |mut store, params, results| {
                    let instance_name = Arc::clone(&instance_name);
                    let func_name = Arc::clone(&func_name);
                    let target = Arc::clone(&target);
                    Box::new(async move {
                        let params: Vec<_> = params
                            .iter()
                            .map(|val| to_wrpc_value(&mut store, val))
                            .collect::<anyhow::Result<_>>()
                            .context("failed to convert wasmtime values to wRPC values")?;
                        let handler = &store.data().handler;
                        let target = handler
                            .identify_interface_target(&target)
                            .await
                            .context("failed to identify interface target")?;
                        let result_values = handler
                            .call(target, &instance_name, &func_name, params)
                            .await
                            .context("failed to call target interface")?;
                        let result_ty = store
                            .data()
                            .custom_result_types
                            .get(instance_name.as_str())
                            .and_then(|instance| instance.get(func_name.as_str()))
                            .context("unknown result type")?;
                        for (i, (val, ty)) in zip(result_values, result_ty).enumerate() {
                            let val = from_wrpc_value(val, ty)?;
                            let result = results.get_mut(i).context("invalid result vector")?;
                            *result = val;
                        }
                        Ok(())
                    })
                },
            ) {
                error!(?err, "failed to polyfill component function import");
            }
            func_import.insert(ty);
        }
        instance_import.insert(function_imports);
    }
    polyfilled_imports
}

#[instrument(level = "trace", skip_all)]
fn instantiate(
    engine: &wasmtime::Engine,
    handler: impl Into<builtin::Handler>,
    ty: types::Component,
    instance_pre: InstancePre<Ctx>,
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
    let mut custom_result_types = HashMap::with_capacity(imports.len());
    for (instance_name, item) in imports {
        match instance_name {
            "wasi:cli/environment@0.2.0"
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
            | "wasi:filesystem/preopens@0.2.0"
            | "wasi:filesystem/types@0.2.0"
            | "wasi:http/incoming-handler@0.2.0"
            | "wasi:http/outgoing-handler@0.2.0"
            | "wasi:http/types@0.2.0"
            | "wasi:io/error@0.2.0"
            | "wasi:io/poll@0.2.0"
            | "wasi:io/streams@0.2.0"
            | "wasi:sockets/tcp@0.2.0"
            | "wasmcloud:bus/lattice"
            | "wasmcloud:bus/guest-config" => continue,
            _ => {}
        }

        let component::types::ComponentItem::ComponentInstance(item) = item else {
            continue;
        };
        let exports = item.exports(engine);
        let mut instance = HashMap::with_capacity(exports.len());
        for (func_name, item) in exports {
            let component::types::ComponentItem::ComponentFunc(ty) = item else {
                continue;
            };
            instance.insert(func_name.to_string(), ty.results().collect());
        }
        if !instance.is_empty() {
            custom_result_types.insert(instance_name.to_string(), instance);
        }
    }

    let handler = handler.into();
    let ctx = Ctx {
        wasi,
        http: WasiHttpCtx,
        table,
        handler,
        stderr,
        custom_result_types,
    };
    let store = wasmtime::Store::new(engine, ctx);
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
        wasmtime_wasi_http::bindings::wasi::http::types::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasi:http/types` interface")?;
        wasmtime_wasi_http::bindings::wasi::http::outgoing_handler::add_to_linker(
            &mut linker,
            |ctx| ctx,
        )
        .context("failed to link `wasi:http/outgoing-handler` interface")?;

        command::add_to_linker(&mut linker).context("failed to link core WASI interfaces")?;

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

        let polyfilled_imports = Arc::new(polyfill(&resolve, imports, &component, &mut linker));
        let ty = linker
            .substituted_component_type(&component)
            .context("failed to introspect component type")?;
        let instance_pre = linker.instantiate_pre(&component)?;
        // TODO: Record the substituted type exports, not parser exports
        Ok(Self {
            engine,
            claims,
            handler: rt.handler.clone(),
            polyfilled_imports,
            exports: Arc::new(function_exports(&resolve, exports)),
            ty,
            instance_pre,
        })
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

    /// Returns a map of dynamic polyfilled function import types.
    /// Top level map is keyed by the instance name.
    /// Inner map is keyed by exported function name.
    #[must_use]
    pub fn polyfilled_imports(&self) -> &Arc<HashMap<String, HashMap<String, DynamicFunction>>> {
        &self.polyfilled_imports
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
        let instance = instantiate(&self.engine, self.handler, self.ty, self.instance_pre)?;
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
        )
    }

    /// Instantiates a [Component] producing an [Instance] and invokes an operation on it using [Instance::call]
    #[instrument(level = "trace", skip_all)]
    pub async fn call(
        &self,
        instance: &str,
        name: &str,
        params: Vec<wrpc_transport::Value>,
    ) -> anyhow::Result<Vec<wrpc_transport::Value>> {
        self.instantiate()
            .context("failed to instantiate component")?
            .call(instance, name, params)
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

    /// Invoke an operation on an [Instance] producing a result.
    #[instrument(skip(self, params, instance, name), fields(interface = instance, function = name))]
    pub async fn call(
        &mut self,
        instance: &str,
        name: &str,
        params: Vec<wrpc_transport::Value>,
    ) -> anyhow::Result<Vec<wrpc_transport::Value>> {
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
            .map(|(val, ty)| from_wrpc_value(val, ty))
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
        results
            .iter()
            .map(|val| to_wrpc_value(&mut self.store, val))
            .collect::<anyhow::Result<_>>()
            .context("failed to convert wasmtime values to wRPC values")
    }
}

/// Instance of a guest interface `T`
pub struct InterfaceInstance<T> {
    store: Mutex<wasmtime::Store<Ctx>>,
    bindings: T,
}
