use crate::capability::{self};
use crate::Runtime;

use core::fmt::{self, Debug};
use core::future::Future;
use core::pin::Pin;
use core::time::Duration;

use anyhow::{ensure, Context as _};
use futures::{Stream, TryStreamExt as _};
use tokio::io::{AsyncRead, AsyncReadExt as _};
use tokio::sync::mpsc;
use tracing::{debug, instrument, warn, Instrument as _, Span};
use wascap::jwt;
use wascap::wasm::extract_claims;
use wasmcloud_component_adapters::WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER;
use wasmtime::component::{types, Linker, ResourceTable, ResourceTableError};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpImpl};
use wrpc_runtime_wasmtime::{
    collect_component_resources, link_item, ServeExt as _, SharedResourceTable, WrpcView,
};

pub use blobstore::HostInputStreamer;
pub use bus::Bus;
pub use config::Config;
pub use logging::Logging;
pub use secrets::Secrets;

mod blobstore;
mod bus;
mod config;
mod http;
mod keyvalue;
mod logging;
mod messaging;
mod secrets;

/// Instance target, which is replaced in wRPC
///
/// This enum represents the original instance import invoked by the component
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ReplacedInstanceTarget {
    /// `wasi:blobstore/blobstore` instance replacement
    BlobstoreBlobstore,
    /// `wasi:blobstore/container` instance replacement
    BlobstoreContainer,
    /// `wasi:keyvalue/atomic` instance replacement
    KeyvalueAtomics,
    /// `wasi:keyvalue/store` instance replacement
    KeyvalueStore,
    /// `wasi:http/incoming-handler` instance replacement
    HttpIncomingHandler,
    /// `wasi:http/outgoing-handler` instance replacement
    HttpOutgoingHandler,
}

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
            | "wasmcloud:messaging/types@0.2.0"
            | "wasmcloud:secrets/store@0.1.0-draft"
            | "wasmcloud:secrets/reveal@0.1.0-draft" => continue,
            _ => {}
        }
    };
}

/// A collection of traits that the host must implement
pub trait Handler:
    wrpc_transport::Invoke<Context = Option<ReplacedInstanceTarget>>
    + Bus
    + Config
    + Logging
    + Secrets
    + Send
    + Sync
    + Clone
    + 'static
{
}

impl<
        T: wrpc_transport::Invoke<Context = Option<ReplacedInstanceTarget>>
            + Bus
            + Config
            + Logging
            + Secrets
            + Send
            + Sync
            + Clone
            + 'static,
    > Handler for T
{
}

/// Component instance configuration
#[derive(Clone, Debug, Default)]
pub struct ComponentConfig {
    /// Whether components are required to be signed to be executed
    pub require_signature: bool,
}

/// Extracts and validates claims contained within a WebAssembly binary, if present
///
/// # Arguments
///
/// * `wasm` - Bytes that constitute a valid WebAssembly binary
///
/// # Errors
///
/// Fails if either parsing fails, or claims are not valid
///
/// # Returns
/// The token embedded in the component, including the [`jwt::Claims`] and the raw JWT
pub fn claims_token(wasm: impl AsRef<[u8]>) -> anyhow::Result<Option<jwt::Token<jwt::Component>>> {
    let Some(claims) = extract_claims(wasm).context("failed to extract module claims")? else {
        return Ok(None);
    };
    let v = jwt::validate_token::<jwt::Component>(&claims.jwt)
        .context("failed to validate module token")?;
    ensure!(!v.expired, "token expired at `{}`", v.expires_human);
    ensure!(
        !v.cannot_use_yet,
        "token cannot be used before `{}`",
        v.not_before_human
    );
    ensure!(v.signature_valid, "signature is not valid");
    Ok(Some(claims))
}

/// Pre-compiled component [Component], which is cheapily-[Cloneable](Clone)
#[derive(Clone)]
pub struct Component<H>
where
    H: Handler,
{
    engine: wasmtime::Engine,
    claims: Option<jwt::Claims<jwt::Component>>,
    instance_pre: wasmtime::component::InstancePre<Ctx<H>>,
    max_execution_time: Duration,
}

impl<H> Debug for Component<H>
where
    H: Handler,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Component")
            .field("claims", &self.claims)
            .field("runtime", &"wasmtime")
            .field("max_execution_time", &self.max_execution_time)
            .finish_non_exhaustive()
    }
}

fn type_annotate_http<F, H>(val: F) -> F
where
    H: Handler,
    F: Fn(&mut Ctx<H>) -> WasiHttpImpl<&mut Ctx<H>>,
{
    val
}

fn new_store<H: Handler>(
    engine: &wasmtime::Engine,
    handler: H,
    max_execution_time: Duration,
) -> wasmtime::Store<Ctx<H>> {
    let table = ResourceTable::new();
    let wasi = WasiCtxBuilder::new()
        .args(&["main.wasm"]) // TODO: Configure argv[0]
        .inherit_stderr()
        .build();

    let mut store = wasmtime::Store::new(
        engine,
        Ctx {
            handler,
            wasi,
            http: WasiHttpCtx::new(),
            table,
            shared_resources: SharedResourceTable::default(),
            timeout: max_execution_time,
        },
    );
    store.set_epoch_deadline(max_execution_time.as_secs());
    store
}

/// Events sent by [`Component::serve_wrpc`]
#[derive(Clone, Debug)]
pub enum WrpcServeEvent<C> {
    /// `wasi:http/incoming-handler.handle` return event
    HttpIncomingHandlerHandleReturned {
        /// Invocation context
        context: C,
        /// Whether the invocation was successfully handled
        success: bool,
    },
    /// `wasmcloud:messaging/handler.handle-message` return event
    MessagingHandlerHandleMessageReturned {
        /// Invocation context
        context: C,
        /// Whether the invocation was successfully handled
        success: bool,
    },
    /// dynamic export return event
    DynamicExportReturned {
        /// Invocation context
        context: C,
        /// Whether the invocation was successfully handled
        success: bool,
    },
}

/// This represents a [Stream] of incoming invocations.
/// Each item represents processing of a single invocation.
pub type InvocationStream = Pin<
    Box<
        dyn Stream<
                Item = anyhow::Result<
                    Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>,
                >,
            > + Send
            + 'static,
    >,
>;

impl<H> Component<H>
where
    H: Handler,
{
    /// Extracts [Claims](jwt::Claims) from WebAssembly component and compiles it using [Runtime].
    ///
    /// If `wasm` represents a core Wasm module, then it will first be turned into a component.
    #[instrument(level = "trace", skip_all)]
    pub fn new(rt: &Runtime, wasm: &[u8]) -> anyhow::Result<Self> {
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
            return Self::new(rt, &wasm);
        }
        let engine = rt.engine.clone();
        let claims_token = claims_token(wasm)?;
        let claims = claims_token.map(|c| c.claims);
        let component = wasmtime::component::Component::new(&engine, wasm)
            .context("failed to compile component")?;

        let mut linker = Linker::new(&engine);

        wasmtime_wasi::add_to_linker_async(&mut linker)
            .context("failed to link core WASI interfaces")?;
        let closure = type_annotate_http(|ctx| WasiHttpImpl(ctx));
        wasmtime_wasi_http::bindings::wasi::http::outgoing_handler::add_to_linker_get_host(
            &mut linker,
            closure,
        )
        .context("failed to link `wasi:http/outgoing-handler` interface")?;
        wasmtime_wasi_http::bindings::wasi::http::types::add_to_linker_get_host(
            &mut linker,
            closure,
        )
        .context("failed to link `wasi:http/types` interface")?;

        capability::blobstore::blobstore::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasi:blobstore/blobstore`")?;
        capability::blobstore::container::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasi:blobstore/container`")?;
        capability::blobstore::types::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasi:blobstore/types`")?;
        capability::config::runtime::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasi:config/runtime`")?;
        capability::keyvalue::atomics::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasi:keyvalue/atomics`")?;
        capability::keyvalue::store::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasi:keyvalue/store`")?;
        capability::logging::logging::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasi:logging/logging`")?;

        capability::bus::lattice::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasmcloud:bus/lattice`")?;
        capability::messaging::consumer::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasmcloud:messaging/consumer`")?;
        capability::secrets::reveal::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasmcloud:secrets/reveal`")?;
        capability::secrets::store::add_to_linker(&mut linker, |ctx| ctx)
            .context("failed to link `wasmcloud:secrets/store`")?;

        let ty = component.component_type();
        let mut guest_resources = Vec::new();
        collect_component_resources(&engine, &ty, &mut guest_resources);
        if !guest_resources.is_empty() {
            warn!("exported component resources are not supported in wasmCloud runtime and will be ignored, use a provider instead to enable this functionality");
        }
        for (name, ty) in ty.imports(&engine) {
            skip_static_instances!(name);
            link_item(&engine, &mut linker.root(), [], ty, "", name, None)
                .context("failed to link item")?;
        }
        let instance_pre = linker.instantiate_pre(&component)?;
        Ok(Self {
            engine,
            claims,
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
    #[instrument(level = "trace", skip_all)]
    pub async fn read(rt: &Runtime, mut wasm: impl AsyncRead + Unpin) -> anyhow::Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf)
            .await
            .context("failed to read Wasm")?;
        Self::new(rt, &buf)
    }

    /// Reads the WebAssembly binary synchronously and calls [Component::new].
    ///
    /// # Errors
    ///
    /// Fails if either reading `wasm` fails or [Self::new] fails
    #[instrument(level = "trace", skip_all)]
    pub fn read_sync(rt: &Runtime, mut wasm: impl std::io::Read) -> anyhow::Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf).context("failed to read Wasm")?;
        Self::new(rt, &buf)
    }

    /// [Claims](jwt::Claims) associated with this [Component].
    #[instrument(level = "trace")]
    pub fn claims(&self) -> Option<&jwt::Claims<jwt::Component>> {
        self.claims.as_ref()
    }

    /// Serve all exports of this [Component] using supplied [`wrpc_transport::Serve`]
    ///
    /// The returned [Vec] contains an [InvocationStream] per each function exported by the component.
    /// A [`WrpcServeEvent`] containing the incoming [`wrpc_transport::Serve::Context`] will be sent
    /// on completion of each invocation.
    /// The supplied [`Handler`] will be used to satisfy imports.
    #[instrument(level = "debug", skip_all)]
    pub async fn serve_wrpc<S>(
        &self,
        srv: &S,
        handler: H,
        events: mpsc::Sender<WrpcServeEvent<S::Context>>,
    ) -> anyhow::Result<Vec<InvocationStream>>
    where
        S: wrpc_transport::Serve,
    {
        let span = Span::current();
        let max_execution_time = self.max_execution_time;
        let mut invocations = vec![];
        let instance = Instance {
            engine: self.engine.clone(),
            pre: self.instance_pre.clone(),
            handler: handler.clone(),
            max_execution_time: self.max_execution_time,
            events: events.clone(),
        };
        for (name, ty) in self
            .instance_pre
            .component()
            .component_type()
            .exports(&self.engine)
        {
            match (name, ty) {
                (
                    "wasi:http/incoming-handler@0.2.0",
                    types::ComponentItem::ComponentInstance(..),
                ) => {
                    let instance = instance.clone();
                    let [(_, _, handle)] = wrpc_interface_http::bindings::exports::wrpc::http::incoming_handler::serve_interface(
                        srv,
                        wrpc_interface_http::ServeWasmtime(instance),
                    )
                    .await
                    .context("failed to serve `wrpc:http/incoming-handler`")?;
                    invocations.push(handle);
                }
                (
                    "wasmcloud:messaging/handler@0.2.0",
                    types::ComponentItem::ComponentInstance(..),
                ) => {
                    let instance = instance.clone();
                    let [(_, _, handle_message)] = messaging::wrpc_handler_bindings::exports::wasmcloud::messaging::handler::serve_interface(
                            srv,
                            instance,
                        )
                        .await
                    .context("failed to serve `wasmcloud:messaging/handler`")?;
                    invocations.push(handle_message);
                }
                (name, types::ComponentItem::ComponentFunc(ty)) => {
                    let engine = self.engine.clone();
                    let handler = handler.clone();
                    let pre = self.instance_pre.clone();
                    debug!(?name, "serving root function");
                    let func = srv
                        .serve_function(
                            move || new_store(&engine, handler.clone(), max_execution_time),
                            pre,
                            ty,
                            "",
                            name,
                        )
                        .await
                        .context("failed to serve root function")?;
                    let events = events.clone();
                    let span = span.clone();
                    invocations.push(Box::pin(func.map_ok(move |(cx, res)| {
                        let events = events.clone();
                        Box::pin(
                            async move {
                                let res = res.await;
                                let success = res.is_ok();
                                if let Err(err) =
                                    events.try_send(WrpcServeEvent::DynamicExportReturned {
                                        context: cx,
                                        success,
                                    })
                                {
                                    warn!(
                                        ?err,
                                        success, "failed to send dynamic root export return event"
                                    );
                                }
                                res
                            }
                            .instrument(span.clone()),
                        )
                            as Pin<Box<dyn Future<Output = _> + Send + 'static>>
                    })));
                }
                (_, types::ComponentItem::CoreFunc(_)) => {
                    warn!(name, "serving root core function exports not supported yet");
                }
                (_, types::ComponentItem::Module(_)) => {
                    warn!(name, "serving root module exports not supported yet");
                }
                (_, types::ComponentItem::Component(_)) => {
                    warn!(name, "serving root component exports not supported yet");
                }
                (instance_name, types::ComponentItem::ComponentInstance(ty)) => {
                    for (name, ty) in ty.exports(&self.engine) {
                        match ty {
                            types::ComponentItem::ComponentFunc(ty) => {
                                let engine = self.engine.clone();
                                let handler = handler.clone();
                                let pre = self.instance_pre.clone();
                                debug!(?instance_name, ?name, "serving instance function");
                                let func = srv
                                    .serve_function(
                                        move || {
                                            new_store(&engine, handler.clone(), max_execution_time)
                                        },
                                        pre,
                                        ty,
                                        instance_name,
                                        name,
                                    )
                                    .await
                                    .context("failed to serve instance function")?;
                                let events = events.clone();
                                let span = span.clone();
                                invocations.push(Box::pin(func.map_ok(move |(cx, res)| {
                                    let events = events.clone();
                                    Box::pin(
                                        async move {
                                            let res = res.await;
                                            let success = res.is_ok();
                                            if let Err(err) = events.try_send(
                                                WrpcServeEvent::DynamicExportReturned {
                                                    context: cx,
                                                    success,
                                                },
                                            ) {
                                                warn!(
                                            ?err,
                                            success,
                                            "failed to send dynamic instance export return event"
                                        );
                                            }
                                            res
                                        }
                                        .instrument(span.clone()),
                                    )
                                        as Pin<Box<dyn Future<Output = _> + Send + 'static>>
                                })));
                            }
                            types::ComponentItem::CoreFunc(_) => {
                                warn!(
                                    instance_name,
                                    name,
                                    "serving instance core function exports not supported yet"
                                );
                            }
                            types::ComponentItem::Module(_) => {
                                warn!(
                                    instance_name,
                                    name, "serving instance module exports not supported yet"
                                );
                            }
                            types::ComponentItem::Component(_) => {
                                warn!(
                                    instance_name,
                                    name, "serving instance component exports not supported yet"
                                );
                            }
                            types::ComponentItem::ComponentInstance(_) => {
                                warn!(
                                    instance_name,
                                    name, "serving nested instance exports not supported yet"
                                );
                            }
                            types::ComponentItem::Type(_) | types::ComponentItem::Resource(_) => {}
                        }
                    }
                }
                (_, types::ComponentItem::Type(_) | types::ComponentItem::Resource(_)) => {}
            }
        }
        Ok(invocations)
    }
}

impl<H> From<Component<H>> for Option<jwt::Claims<jwt::Component>>
where
    H: Handler,
{
    fn from(Component { claims, .. }: Component<H>) -> Self {
        claims
    }
}

struct Instance<H, C>
where
    H: Handler,
{
    engine: wasmtime::Engine,
    pre: wasmtime::component::InstancePre<Ctx<H>>,
    handler: H,
    max_execution_time: Duration,
    events: mpsc::Sender<WrpcServeEvent<C>>,
}

impl<H, C> Clone for Instance<H, C>
where
    H: Handler,
{
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            pre: self.pre.clone(),
            handler: self.handler.clone(),
            max_execution_time: self.max_execution_time,
            events: self.events.clone(),
        }
    }
}

type TableResult<T> = Result<T, ResourceTableError>;

struct Ctx<H>
where
    H: Handler,
{
    handler: H,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    shared_resources: SharedResourceTable,
    timeout: Duration,
}

impl<H: Handler> WasiView for Ctx<H> {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}

impl<H: Handler> WrpcView for Ctx<H> {
    type Invoke = H;

    fn client(&self) -> &H {
        &self.handler
    }

    fn shared_resources(&mut self) -> &mut SharedResourceTable {
        &mut self.shared_resources
    }

    fn timeout(&self) -> Option<Duration> {
        Some(self.timeout)
    }
}

impl<H: Handler> Debug for Ctx<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx").field("runtime", &"wasmtime").finish()
    }
}
