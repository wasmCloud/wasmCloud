use crate::actor::claims;
use crate::capability::{builtin, Bus, Interfaces, TargetInterface};
use crate::Runtime;

use core::fmt::{self, Debug};
use core::iter::zip;
use core::mem::replace;
use core::ops::{Deref, DerefMut};

use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context as _};
use async_trait::async_trait;
use bytes::Bytes;
use tokio::io::AsyncWrite;
use tokio::sync::Mutex;
use tracing::{error, instrument, trace};
use wascap::jwt;
use wasmtime::component::{self, Linker, ResourceTable, ResourceTableError, Val};
use wasmtime_wasi::preview2::command::{self, Command};
use wasmtime_wasi::preview2::pipe::{
    AsyncReadStream, AsyncWriteStream, ClosedInputStream, ClosedOutputStream,
};
use wasmtime_wasi::preview2::{
    HostInputStream, HostOutputStream, StdinStream, StdoutStream, StreamError, StreamResult,
    Subscribe, WasiCtx, WasiCtxBuilder, WasiView,
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
    stdin: StdioStream<Box<dyn HostInputStream>>,
    stdout: StdioStream<Box<dyn HostOutputStream>>,
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

fn to_wrpc_value(val: &Val) -> anyhow::Result<wrpc_transport::Value> {
    match val {
        Val::Bool(val) => Ok(wrpc_transport::Value::Bool(*val)),
        Val::S8(val) => Ok(wrpc_transport::Value::S8(*val)),
        Val::U8(val) => Ok(wrpc_transport::Value::U8(*val)),
        Val::S16(val) => Ok(wrpc_transport::Value::S16(*val)),
        Val::U16(val) => Ok(wrpc_transport::Value::U16(*val)),
        Val::S32(val) => Ok(wrpc_transport::Value::S32(*val)),
        Val::U32(val) => Ok(wrpc_transport::Value::U32(*val)),
        Val::S64(val) => Ok(wrpc_transport::Value::S64(*val)),
        Val::U64(val) => Ok(wrpc_transport::Value::U64(*val)),
        Val::Float32(val) => Ok(wrpc_transport::Value::Float32(*val)),
        Val::Float64(val) => Ok(wrpc_transport::Value::Float64(*val)),
        Val::Char(val) => Ok(wrpc_transport::Value::Char(*val)),
        Val::String(val) => Ok(wrpc_transport::Value::String(val.to_string())),
        _ => bail!("complex types not supported yet"),
    }
}

// TODO: Remove this in wasmtime 18
fn from_wrpc_value_simple(val: wrpc_transport::Value) -> anyhow::Result<Val> {
    match val {
        wrpc_transport::Value::Bool(v) => Ok(Val::Bool(v)),
        wrpc_transport::Value::U8(v) => Ok(Val::U8(v)),
        wrpc_transport::Value::U16(v) => Ok(Val::U16(v)),
        wrpc_transport::Value::U32(v) => Ok(Val::U32(v)),
        wrpc_transport::Value::U64(v) => Ok(Val::U64(v)),
        wrpc_transport::Value::S8(v) => Ok(Val::S8(v)),
        wrpc_transport::Value::S16(v) => Ok(Val::S16(v)),
        wrpc_transport::Value::S32(v) => Ok(Val::S32(v)),
        wrpc_transport::Value::S64(v) => Ok(Val::S64(v)),
        wrpc_transport::Value::Float32(v) => Ok(Val::Float32(v)),
        wrpc_transport::Value::Float64(v) => Ok(Val::Float64(v)),
        wrpc_transport::Value::Char(v) => Ok(Val::Char(v)),
        wrpc_transport::Value::String(v) => Ok(Val::String(v.into())),
        _ => bail!("complex types not supported yet"),
    }
}

fn from_wrpc_value(val: wrpc_transport::Value, ty: &component::Type) -> anyhow::Result<Val> {
    use component::Type;

    match (val, ty) {
        (wrpc_transport::Value::Bool(v), Type::Bool) => Ok(Val::Bool(v)),
        (wrpc_transport::Value::U8(v), Type::U8) => Ok(Val::U8(v)),
        (wrpc_transport::Value::U16(v), Type::U16) => Ok(Val::U16(v)),
        (wrpc_transport::Value::U32(v), Type::U32) => Ok(Val::U32(v)),
        (wrpc_transport::Value::U64(v), Type::U64) => Ok(Val::U64(v)),
        (wrpc_transport::Value::S8(v), Type::S8) => Ok(Val::S8(v)),
        (wrpc_transport::Value::S16(v), Type::S16) => Ok(Val::S16(v)),
        (wrpc_transport::Value::S32(v), Type::S32) => Ok(Val::S32(v)),
        (wrpc_transport::Value::S64(v), Type::S64) => Ok(Val::S64(v)),
        (wrpc_transport::Value::Float32(v), Type::Float32) => Ok(Val::Float32(v)),
        (wrpc_transport::Value::Float64(v), Type::Float64) => Ok(Val::Float64(v)),
        (wrpc_transport::Value::Char(v), Type::Char) => Ok(Val::Char(v)),
        (wrpc_transport::Value::String(v), Type::String) => Ok(Val::String(v.into())),
        (wrpc_transport::Value::List(vs), Type::List(ty)) => {
            let mut w_vs = Vec::with_capacity(vs.len());
            let el_ty = ty.ty();
            for v in vs {
                let v = from_wrpc_value(v, &el_ty).context("failed to convert list element")?;
                w_vs.push(v);
            }
            component::List::new(ty, w_vs.into()).map(component::Val::List)
        }
        (wrpc_transport::Value::Record(vs), Type::Record(ty)) => {
            let mut w_vs = Vec::with_capacity(vs.len());
            for (v, component::types::Field { name, ty }) in zip(vs, ty.fields()) {
                let v = from_wrpc_value(v, &ty).context("failed to convert record field")?;
                w_vs.push((name, v));
            }
            component::Record::new(ty, w_vs).map(component::Val::Record)
        }
        (wrpc_transport::Value::Tuple(vs), Type::Tuple(ty)) => {
            let mut w_vs = Vec::with_capacity(vs.len());
            for (v, ty) in zip(vs, ty.types()) {
                let v = from_wrpc_value(v, &ty).context("failed to convert tuple element")?;
                w_vs.push(v);
            }
            component::Tuple::new(ty, w_vs.into()).map(component::Val::Tuple)
        }
        (
            wrpc_transport::Value::Variant {
                discriminant,
                nested,
            },
            Type::Variant(ty),
        ) => {
            let discriminant = discriminant
                .try_into()
                .context("discriminant does not fit in usize")?;
            let component::types::Case { name, ty: case_ty } = ty
                .cases()
                .skip(discriminant)
                .next()
                .context("variant discriminant not found")?;
            let v = if let Some(case_ty) = case_ty {
                let v = nested.context("nested value missing")?;
                let v = from_wrpc_value(*v, &case_ty).context("failed to convert variant value")?;
                Some(v)
            } else {
                None
            };
            component::Variant::new(ty, name, v).map(component::Val::Variant)
        }
        (wrpc_transport::Value::Enum(discriminant), Type::Enum(ty)) => {
            let discriminant = discriminant
                .try_into()
                .context("discriminant does not fit in usize")?;
            let name = ty
                .names()
                .skip(discriminant)
                .next()
                .context("enum discriminant not found")?;
            component::Enum::new(ty, name).map(component::Val::Enum)
        }
        (wrpc_transport::Value::Option(v), Type::Option(ty)) => {
            let v = if let Some(v) = v {
                let v = from_wrpc_value(*v, &ty.ty()).context("failed to convert option value")?;
                Some(v)
            } else {
                None
            };
            component::OptionVal::new(ty, v).map(component::Val::Option)
        }
        (wrpc_transport::Value::Result(v), component::Type::Result(ty)) => {
            let v = match v {
                Ok(None) => Ok(None),
                Ok(Some(v)) => {
                    let ty = ty.ok().context("`result::ok` type missing")?;
                    let v =
                        from_wrpc_value(*v, &ty).context("failed to convert `result::ok` value")?;
                    Ok(Some(v))
                }
                Err(None) => Err(None),
                Err(Some(v)) => {
                    let ty = ty.err().context("`result::err` type missing")?;
                    let v = from_wrpc_value(*v, &ty)
                        .context("failed to convert `result::err` value")?;
                    Err(Some(v))
                }
            };
            component::ResultVal::new(ty, v).map(component::Val::Result)
        }
        (wrpc_transport::Value::Flags(v), Type::Flags(ty)) => {
            // NOTE: Currently flags are limited to 64
            let mut names = Vec::with_capacity(64);
            for (i, name) in zip(0..64, ty.names()) {
                if v & (1 << i) != 0 {
                    names.push(name)
                }
            }
            component::Flags::new(ty, &names).map(component::Val::Flags)
        }
        _ => bail!("type mismatch"),
    }
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
        let instance_name = Arc::new(resolve.name_world_key(key));
        match instance_name.as_str() {
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
            | "wasi:sockets/tcp@0.2.0" => continue,
            _ => {}
        }
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
            ("wasmcloud", "bus" | "messaging") => continue,
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
                for (name, _) in interface.functions.iter().filter(|(name, function)| {
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
                    let instance_name = Arc::clone(&instance_name);
                    let name = Arc::new(name.to_string());
                    let target = Arc::clone(&target);
                    if let Err(err) = linker.func_new_async(
                        component,
                        Arc::clone(&name).as_str(),
                        move |ctx, params, results| {
                            let instance_name = Arc::clone(&instance_name);
                            let name = Arc::clone(&name);
                            let target = Arc::clone(&target);
                            Box::new(async move {
                                let params: Vec<_> = params
                                    .into_iter()
                                    .map(|param| to_wrpc_value(param))
                                    .collect::<anyhow::Result<_>>()
                                    .context("failed to convert wasmtime values to wRPC values")?;
                                let handler = &ctx.data().handler;
                                let target = handler
                                    .identify_interface_target(&target)
                                    .await
                                    .context("failed to identify interface target")?;
                                let result_values = handler
                                    .call(target, &instance_name, &name, params)
                                    .await
                                    .context("failed to call target")?;
                                for (i, val) in result_values.into_iter().enumerate() {
                                    let val = from_wrpc_value_simple(val)?;
                                    let result =
                                        results.get_mut(i).context("invalid result vector")?;
                                    *result = val;
                                }
                                Ok(())
                            })
                        },
                    ) {
                        error!(
                            ?err,
                            namespace = package.name.namespace,
                            package = package.name.name,
                            interface = interface_name,
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

    let table = ResourceTable::new();
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

    /// Invoke an operation on an [Instance] producing a result.
    #[instrument(skip_all)]
    pub async fn call(
        &mut self,
        instance: &str,
        name: &str,
        params: Vec<wrpc_transport::Value>,
    ) -> anyhow::Result<Vec<wrpc_transport::Value>> {
        let component = self
            .linker
            .instantiate_async(&mut self.store, &self.component)
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
            .map(to_wrpc_value)
            .collect::<anyhow::Result<_>>()
            .context("failed to convert wasmtime values to wRPC values")
    }
}

/// Instance of a guest interface `T`
pub struct InterfaceInstance<T> {
    store: Mutex<wasmtime::Store<Ctx>>,
    bindings: T,
}
