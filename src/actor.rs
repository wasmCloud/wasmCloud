use crate::capability::{Logging, Numbergen, Provider};
use crate::Runtime;

use core::fmt::{self, Debug};
use core::ptr::NonNull;

use anyhow::{bail, ensure, Context, Result};
use futures::AsyncReadExt;
use tracing::{instrument, trace, trace_span, warn};
use wascap::{jwt, wasm::extract_claims};
use wasmbus_rpc::common::{deserialize, serialize};
use wasmcloud_interface_logging::LogEntry;
use wasmcloud_interface_numbergen::RangeLimit;

mod wasm {
    #[allow(non_camel_case_types)]
    pub type ptr = i32;
    #[allow(non_camel_case_types)]
    pub type usize = i32;

    pub const ERROR: usize = usize::MAX;
    pub const SUCCESS: usize = 1;
}

mod guest_call {
    use super::{wasm, NonNull};

    pub type Params = (wasm::usize, wasm::usize);
    pub type Result = wasm::usize;

    pub type State = (NonNull<[u8]>, NonNull<[u8]>);
}

struct Ctx<'a, L, N, P> {
    wasi: wasmtime_wasi::WasiCtx,
    claims: &'a jwt::Claims<jwt::Actor>,
    console_log: Vec<String>,
    guest_call: Option<guest_call::State>,
    guest_error: Option<String>,
    guest_response: Option<Vec<u8>>,
    host_error: Option<String>,
    host_response: Option<Vec<u8>>,
    logging: L,
    numbergen: N,
    provider: P,
}

impl<L, N, P> Debug for Ctx<'_, L, N, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("runtime", &"wasmtime")
            .field("claims", &self.claims)
            .field("console_log", &self.console_log)
            .field("guest_call", &self.guest_call)
            .field("guest_error", &self.guest_error)
            .field("guest_response", &self.guest_response)
            .field("host_error", &self.host_error)
            .field("host_response", &self.host_response)
            .finish()
    }
}

impl<'a, L, N, P> Ctx<'a, L, N, P> {
    fn new(
        claims: &'a jwt::Claims<jwt::Actor>,
        logging: L,
        numbergen: N,
        provider: P,
    ) -> Result<Self> {
        // TODO: Set stdio pipes
        let wasi = wasmtime_wasi::WasiCtxBuilder::new()
            .arg("main.wasm")
            .context("failed to set argv[0]")?
            .build();
        Ok(Self {
            wasi,
            claims,
            console_log: Vec::default(),
            guest_call: None,
            guest_error: None,
            guest_response: None,
            host_error: None,
            host_response: None,
            logging,
            numbergen,
            provider,
        })
    }

    fn reset(&mut self) {
        self.console_log = Vec::default();
        self.guest_call = None;
        self.guest_error = None;
        self.guest_response = None;
        self.host_error = None;
        self.host_response = None;
    }
}

fn caller_memory<T>(store: &mut wasmtime::Caller<'_, T>) -> wasmtime::Memory {
    store
        .get_export("memory")
        .expect("`memory` not defined")
        .into_memory()
        .expect("`memory` type is not valid")
}

#[instrument(skip(store, memory))]
fn read_bytes<T>(
    store: &mut wasmtime::Caller<'_, T>,
    memory: &wasmtime::Memory,
    data: wasm::ptr,
    len: wasm::usize,
) -> Result<Vec<u8>> {
    let data = data.try_into().context("pointer does not fit in usize")?;
    let len = len.try_into().context("size does not fit in usize")?;

    let mut buf = vec![0; len];
    memory
        .read(store, data, &mut buf)
        .context("failed to read data from guest memory")?;
    trace!(?buf, "read buffer from guest memory");
    Ok(buf)
}

#[instrument(skip(store, memory), fields(result))]
fn read_string<T>(
    store: &mut wasmtime::Caller<'_, T>,
    memory: &wasmtime::Memory,
    data: wasm::ptr,
    len: wasm::usize,
) -> Result<String> {
    let buf =
        read_bytes(store, memory, data, len).context("failed to read bytes from guest memory")?;
    let s = String::from_utf8(buf).context("failed to parse bytes as UTF-8")?;
    trace!(s, "read string from guest memory");
    Ok(s)
}

fn write_bytes<T>(
    store: &mut wasmtime::Caller<'_, T>,
    memory: &wasmtime::Memory,
    data: wasm::ptr,
    buf: impl AsRef<[u8]>,
) -> Result<()> {
    let buf = buf.as_ref();
    let data = data.try_into().context("pointer does not fit in usize")?;
    memory
        .write(store, data, buf)
        .context("failed to write bytes to guest memory")?;
    trace!(?buf, "wrote bytes into guest memory");
    Ok(())
}

#[instrument(skip(store, err))]
fn set_host_error<L, N, P>(store: &mut wasmtime::Caller<'_, Ctx<L, N, P>>, err: impl ToString) {
    let err = err.to_string();
    trace!(err, "set host error");
    store.data_mut().host_error = Some(err);
}

#[instrument(skip(store, res))]
fn set_host_response<L, N, P>(
    store: &mut wasmtime::Caller<'_, Ctx<L, N, P>>,
    res: impl Into<Vec<u8>>,
) {
    let res = res.into();
    trace!(?res, "set host response");
    store.data_mut().host_response = Some(res);
}

#[instrument(skip(store))]
fn console_log<L, N, P>(
    mut store: wasmtime::Caller<'_, Ctx<L, N, P>>,
    log_ptr: wasm::ptr,
    log_len: wasm::usize,
) -> Result<()> {
    let memory = caller_memory(&mut store);
    let log = read_string(&mut store, &memory, log_ptr, log_len)
        .context("failed to read `__console_log` log string")?;
    trace!(log, "store log string");
    store.data_mut().console_log.push(log);
    Ok(())
}

#[instrument(skip(store))]
fn guest_error<L, N, P>(
    mut store: wasmtime::Caller<'_, Ctx<L, N, P>>,
    err_ptr: wasm::ptr,
    err_len: wasm::usize,
) -> Result<()> {
    let memory = caller_memory(&mut store);
    let err = read_string(&mut store, &memory, err_ptr, err_len)
        .context("failed to read `__guest_error` error string")?;
    trace!(err, "set guest error");
    store.data_mut().guest_error = Some(err);
    Ok(())
}

#[instrument(skip(store))]
fn guest_request<L, N, P>(
    mut store: wasmtime::Caller<'_, Ctx<L, N, P>>,
    op_ptr: wasm::ptr,
    pld_ptr: wasm::ptr,
) -> Result<()> {
    let (op, pld) = store
        .data_mut()
        .guest_call
        .take()
        .context("unexpected `__guest_request`")?;

    let memory = caller_memory(&mut store);
    write_bytes(&mut store, &memory, op_ptr, unsafe { op.as_ref() })
        .context("failed to write `__guest_call` operation into guest memory")?;
    write_bytes(&mut store, &memory, pld_ptr, unsafe { pld.as_ref() })
        .context("failed to write `__guest_call` payload into guest memory")
}

#[instrument(skip(store))]
fn guest_response<L, N, P>(
    mut store: wasmtime::Caller<'_, Ctx<L, N, P>>,
    res_ptr: wasm::ptr,
    res_len: wasm::usize,
) -> Result<()> {
    let memory = caller_memory(&mut store);
    let res = read_bytes(&mut store, &memory, res_ptr, res_len)
        .context("failed to read `__guest_response` response")?;
    trace!(?res, "set guest response");
    store.data_mut().guest_response = Some(res);
    Ok(())
}

trait ProviderResult {
    fn into_wasm<L, N, P>(
        self,
        store: &mut wasmtime::Caller<'_, Ctx<L, N, P>>,
    ) -> Result<wasm::usize>;
}

impl<E: ToString> ProviderResult for core::result::Result<(), E> {
    fn into_wasm<L, N, P>(
        self,
        store: &mut wasmtime::Caller<'_, Ctx<L, N, P>>,
    ) -> Result<wasm::usize> {
        if let Err(err) = self {
            set_host_error(store, err);
            Ok(wasm::ERROR)
        } else {
            set_host_response(store, []);
            Ok(wasm::SUCCESS)
        }
    }
}

impl<E: ToString> ProviderResult for core::result::Result<Vec<u8>, E> {
    fn into_wasm<L, N, P>(
        self,
        store: &mut wasmtime::Caller<'_, Ctx<L, N, P>>,
    ) -> Result<wasm::usize> {
        match self {
            Ok(buf) => {
                set_host_response(store, buf);
                Ok(wasm::SUCCESS)
            }
            Err(err) => {
                set_host_error(store, err);
                Ok(wasm::ERROR)
            }
        }
    }
}

#[instrument(skip(store))]
#[allow(clippy::too_many_arguments)]
fn host_call<L: Logging, N: Numbergen, P: Provider>(
    mut store: wasmtime::Caller<'_, Ctx<L, N, P>>,
    bd_ptr: wasm::ptr,
    bd_len: wasm::usize,
    ns_ptr: wasm::ptr,
    ns_len: wasm::usize,
    op_ptr: wasm::ptr,
    op_len: wasm::usize,
    pld_ptr: wasm::ptr,
    pld_len: wasm::usize,
) -> Result<wasm::usize> {
    let memory = caller_memory(&mut store);
    let bd = read_string(&mut store, &memory, bd_ptr, bd_len)
        .context("failed to read `__host_call` binding")?;

    let ns = read_string(&mut store, &memory, ns_ptr, ns_len)
        .context("failed to read `__host_call` namespace")?;

    let op = read_string(&mut store, &memory, op_ptr, op_len)
        .context("failed to read `__host_call` operation")?;

    let pld = read_bytes(&mut store, &memory, pld_ptr, pld_len)
        .context("failed to read `__host_call` payload")?;

    ensure!(
        store
            .data()
            .claims
            .metadata
            .as_ref()
            .map(|jwt::Actor { caps, .. }| caps.as_ref().map(|caps| caps.contains(&ns)))
            .unwrap_or_default()
            .unwrap_or_default(),
        "`{ns}` capability request unauthorized"
    );

    trace_span!("call provider", bd, ns, op, ?pld).in_scope(|| {
        match (bd.as_str(), ns.as_str(), op.as_str()) {
            (_, "wasmcloud:builtin:logging", "Logging.WriteLog") => {
                let LogEntry { level, text } =
                    deserialize(&pld).context("failed to deserialize log entry")?;
                match level.as_str() {
                    "debug" => trace_span!("Logging::debug")
                        .in_scope(|| store.data().logging.debug(text))
                        .into_wasm(&mut store),
                    "info" => trace_span!("Logging::info")
                        .in_scope(|| store.data().logging.info(text))
                        .into_wasm(&mut store),
                    "warn" => trace_span!("Logging::warn")
                        .in_scope(|| store.data().logging.warn(text))
                        .into_wasm(&mut store),
                    "error" => trace_span!("Logging::error")
                        .in_scope(|| store.data().logging.error(text))
                        .into_wasm(&mut store),
                    _ => {
                        bail!("log level `{level}` is not supported")
                    }
                }
            }
            (_, "wasmcloud:builtin:numbergen", "NumberGen.GenerateGuid") => {
                match trace_span!("Numbergen::generate_guid")
                    .in_scope(|| store.data().numbergen.generate_guid())
                {
                    Ok(guid) => serialize(&guid.to_string()).into_wasm(&mut store),
                    Err(err) => {
                        set_host_error(&mut store, err);
                        Ok(wasm::ERROR)
                    }
                }
            }
            (_, "wasmcloud:builtin:numbergen", "NumberGen.RandomInRange") => {
                let RangeLimit { min, max } =
                    deserialize(&pld).context("failed to deserialize range limit")?;
                match trace_span!("Numbergen::random_in_range")
                    .in_scope(|| store.data().numbergen.random_in_range(min, max))
                {
                    Ok(v) => serialize(&v).into_wasm(&mut store),
                    Err(err) => {
                        set_host_error(&mut store, err);
                        Ok(wasm::ERROR)
                    }
                }
            }
            (_, "wasmcloud:builtin:numbergen", "NumberGen.Random32") => {
                match trace_span!("Numbergen::random_32")
                    .in_scope(|| store.data().numbergen.random_32())
                {
                    Ok(v) => serialize(&v).into_wasm(&mut store),
                    Err(err) => {
                        set_host_error(&mut store, err);
                        Ok(wasm::ERROR)
                    }
                }
            }
            _ => trace_span!("Provider::handle").in_scope(|| {
                store
                    .data()
                    .provider
                    .handle(bd, ns, op, pld)
                    .into_wasm(&mut store)
            }),
        }
    })
}

#[instrument(skip(store))]
fn host_error<L, N, P>(
    mut store: wasmtime::Caller<'_, Ctx<L, N, P>>,
    err_ptr: wasm::ptr,
) -> Result<()> {
    let err = store
        .data_mut()
        .host_error
        .take()
        .context("unexpected `__host_error`")?;

    let memory = caller_memory(&mut store);
    trace_span!("write error into guest memory", err)
        .in_scope(|| write_bytes(&mut store, &memory, err_ptr, err.as_bytes()))
        .context("failed to write `__host_error` error into guest memory")
}

#[instrument(skip(store))]
fn host_error_len<L, N, P>(store: wasmtime::Caller<'_, Ctx<L, N, P>>) -> wasm::usize {
    let len = store
        .data()
        .host_error
        .as_ref()
        .map(String::as_bytes)
        .map(<[_]>::len)
        .unwrap_or_default()
        .try_into()
        .unwrap_or_else(|_| {
            warn!(
                "`host_error_len` does not fit in i32, truncating to {}",
                wasm::usize::MAX
            );
            wasm::usize::MAX
        });
    trace!(len);
    len
}

#[instrument(skip(store))]
fn host_response<L, N, P>(
    mut store: wasmtime::Caller<'_, Ctx<L, N, P>>,
    res_ptr: wasm::ptr,
) -> Result<()> {
    let res = store
        .data_mut()
        .host_response
        .take()
        .context("unexpected `__host_response`")?;

    let memory = caller_memory(&mut store);
    trace_span!("write response into guest memory", ?res)
        .in_scope(|| write_bytes(&mut store, &memory, res_ptr, res))
        .context("failed to write `__host_response` response into guest memory")
}

#[instrument(skip(store))]
fn host_response_len<L, N, P>(store: wasmtime::Caller<'_, Ctx<L, N, P>>) -> wasm::usize {
    let len = store
        .data()
        .host_response
        .as_ref()
        .map(Vec::len)
        .unwrap_or_default()
        .try_into()
        .unwrap_or_else(|_| {
            warn!(
                "`host_response_len` does not fit in i32, truncating to {}",
                wasm::usize::MAX
            );
            wasm::usize::MAX
        });
    trace!(len);
    len
}

/// Actor module instance config used by [`Module::instantiate`]
pub struct InstanceConfig {
    /// Minimum amount of WebAssembly memory pages to allocate for an actor instance.
    ///
    /// A WebAssembly memory page size is 64k.
    pub min_memory_pages: u32,
    /// WebAssembly memory page allocation limit for an actor instance.
    ///
    /// A WebAssembly memory page size is 64k.
    pub max_memory_pages: Option<u32>,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            min_memory_pages: 4,
            max_memory_pages: None,
        }
    }
}

/// Pre-compiled actor [Module], which is cheapily-[Cloneable](Clone)
#[derive(Clone)]
pub struct Module {
    module: wasmtime::Module,
    claims: jwt::Claims<jwt::Actor>,
}

impl Debug for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Actor")
            .field("runtime", &"wasmtime")
            .field("claims", &self.claims)
            .finish()
    }
}

impl Module {
    /// Extracts [Claims](jwt::Claims) from WebAssembly module and compiles it using [Runtime].
    #[instrument(skip(wasm))]
    pub fn new(rt: &Runtime, wasm: impl AsRef<[u8]>) -> Result<Self> {
        let wasm = wasm.as_ref();

        let claims = extract_claims(wasm)
            .context("failed to extract module claims")?
            .context("execution of unsigned Wasm modules is not allowed")?;
        let v = jwt::validate_token::<jwt::Actor>(&claims.jwt)
            .context("failed to validate module token")?;
        ensure!(!v.expired, "token expired at `{}`", v.expires_human);
        ensure!(
            !v.cannot_use_yet,
            "token cannot be used before `{}`",
            v.not_before_human
        );
        ensure!(v.signature_valid, "signature is not valid");

        let module = wasmtime::Module::new(&rt.engine, wasm).context("failed to compile module")?;
        Ok(Self {
            module,
            claims: claims.claims,
        })
    }

    /// [Claims](jwt::Claims) associated with this [Module].
    #[instrument]
    pub fn claims(&self) -> &jwt::Claims<jwt::Actor> {
        &self.claims
    }

    /// Reads the WebAssembly module asynchronously and calls [Module::new].
    #[instrument(skip(wasm))]
    pub async fn read_async(
        rt: &Runtime,
        mut wasm: impl futures::AsyncRead + Unpin,
    ) -> Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf)
            .await
            .context("failed to read Wasm")?;
        Self::new(rt, buf)
    }

    /// Reads the WebAssembly module synchronously and calls [Module::new].
    #[instrument(skip(wasm))]
    pub fn read(rt: &Runtime, mut wasm: impl std::io::Read) -> Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf).context("failed to read Wasm")?;
        Self::new(rt, buf)
    }

    /// Instantiates a [Module] given an [InstanceConfig] and returns the resulting [Instance].
    #[instrument(skip_all)]
    pub fn instantiate<L, N, P>(
        &self,
        InstanceConfig {
            min_memory_pages,
            max_memory_pages,
        }: InstanceConfig,
        logging: L,
        numbergen: N,
        provider: P,
    ) -> Result<Instance<L, N, P>>
    where
        L: Logging + 'static,
        N: Numbergen + 'static,
        P: Provider + 'static,
    {
        let engine = self.module.engine();

        let cx = Ctx::new(&self.claims, logging, numbergen, provider)
            .context("failed to construct store context")?;
        let mut store = wasmtime::Store::new(engine, cx);
        let mut linker = wasmtime::Linker::<Ctx<L, N, P>>::new(engine);

        wasmtime_wasi::add_to_linker(&mut linker, |cx| &mut cx.wasi)
            .context("failed to link WASI")?;

        linker.func_wrap("wasmbus", "__console_log", console_log)?;
        linker.func_wrap("wasmbus", "__guest_error", guest_error)?;
        linker.func_wrap("wasmbus", "__guest_request", guest_request)?;
        linker.func_wrap("wasmbus", "__guest_response", guest_response)?;
        linker.func_wrap("wasmbus", "__host_call", host_call)?;
        linker.func_wrap("wasmbus", "__host_error", host_error)?;
        linker.func_wrap("wasmbus", "__host_error_len", host_error_len)?;
        linker.func_wrap("wasmbus", "__host_response", host_response)?;
        linker.func_wrap("wasmbus", "__host_response_len", host_response_len)?;

        // TODO: allow configuration of min and max memory pages
        let memory = wasmtime::Memory::new(
            &mut store,
            wasmtime::MemoryType::new(min_memory_pages, max_memory_pages),
        )
        .context("failed to initialize memory")?;
        linker
            .define_name(&store, "memory", memory)
            .context("failed to define `memory`")?;

        let instance = linker
            .instantiate(&mut store, &self.module)
            .context("failed to instantiate module")?;

        // TODO: call start etc.

        let func = instance
            .get_typed_func(&mut store, "__guest_call")
            .context("failed to get `__guest_call` export")?;
        Ok(Instance { func, store })
    }
}

/// An instance of a [Module]
pub struct Instance<'a, L, N, P> {
    func: wasmtime::TypedFunc<guest_call::Params, guest_call::Result>,
    store: wasmtime::Store<Ctx<'a, L, N, P>>,
}

/// An actor [Instance] operation result returned in response to [`Instance::call`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Response {
    /// Code returned by an invocation of an operation on an actor [Instance].
    pub code: i32,
    /// Binary guest operation invocation response if returned by the guest.
    pub response: Option<Vec<u8>>,
    /// Console logs produced by a [Instance] operation invocation. Note, that this functionality
    /// is deprecated and should be empty in most cases.
    pub console_log: Vec<String>,
}

impl<L, N, P> Instance<'_, L, N, P> {
    /// Invoke an operation on an [Instance] producing a [Response].
    #[instrument(skip_all)]
    pub fn call(
        &mut self,
        operation: impl AsRef<str>,
        payload: impl AsRef<[u8]>,
    ) -> Result<Response> {
        self.store.data_mut().reset();

        let operation = operation.as_ref().as_bytes();
        let operation_len = operation
            .len()
            .try_into()
            .context("operation string length does not fit in i32")?;

        let payload = payload.as_ref();
        let payload_len = payload
            .len()
            .try_into()
            .context("payload length does not fit in i32")?;

        self.store.data_mut().guest_call = Some((operation.into(), payload.into()));

        let code = self
            .func
            .call(&mut self.store, (operation_len, payload_len))
            .context("failed to call `__guest_call`")?;
        let store = self.store.data_mut();
        if let Some(err) = store.guest_error.take() {
            bail!(err)
        } else if let Some(err) = store.host_error.take() {
            bail!(err)
        }
        let response = store.guest_response.take();
        let console_log = store.console_log.drain(..).collect();
        Ok(Response {
            code,
            response,
            console_log,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::capability::Uuid;
    use crate::{capability, ActorInstanceConfig, ActorModule, ActorResponse, Runtime};

    use std::convert::Infallible;

    use anyhow::Context;
    use once_cell::sync::Lazy;
    use serde::Deserialize;
    use serde_json::json;
    use tracing_subscriber::prelude::*;
    use wascap::caps;
    use wascap::prelude::{ClaimsBuilder, KeyPair};
    use wascap::wasm::embed_claims;
    use wasmbus_rpc::common::{deserialize, serialize};
    use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse};

    static LOGGER: Lazy<()> = Lazy::new(|| {
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().pretty().without_time())
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new(
                        "info,integration=trace,wasmcloud=trace,cranelift_codegen=warn",
                    )
                }),
            )
            .init();
    });
    static UUID: Lazy<Uuid> = Lazy::new(Uuid::new_v4);

    static RUNTIME: Lazy<Runtime> = Lazy::new(|| Runtime::builder().into());

    static HTTP_LOG_RNG_REQUEST: Lazy<Vec<u8>> = Lazy::new(|| {
        let body = serde_json::to_vec(&json!({
            "min": 42,
            "max": 4242,
        }))
        .expect("failed to encode body to JSON");
        serialize(&HttpRequest {
            body,
            ..Default::default()
        })
        .expect("failed to serialize request")
    });
    static HTTP_LOG_RNG_MODULE: Lazy<Module> = Lazy::new(|| {
        let wasm = std::fs::read(env!("CARGO_CDYLIB_FILE_ACTOR_HTTP_LOG_RNG"))
            .expect("failed to read `{HTTP_LOG_RNG_WASM}`");

        let issuer = KeyPair::new_account();
        let module = KeyPair::new_module();

        let claims = ClaimsBuilder::new()
            .issuer(&issuer.public_key())
            .subject(&module.public_key())
            .with_metadata(jwt::Actor::default()) // this will be overriden by individual test cases
            .build();
        let wasm = embed_claims(&wasm, &claims, &issuer).expect("failed to embed actor claims");

        let actor =
            ActorModule::read(&RUNTIME, wasm.as_slice()).expect("failed to read actor module");

        assert_eq!(actor.claims().subject, module.public_key());

        actor
    });

    struct Logging;
    impl capability::Logging for Logging {
        type Error = Infallible;

        fn debug(&self, text: String) -> Result<(), Self::Error> {
            assert_eq!(text, "debug");
            Ok(())
        }
        fn info(&self, text: String) -> Result<(), Self::Error> {
            assert_eq!(text, "info");
            Ok(())
        }
        fn warn(&self, text: String) -> Result<(), Self::Error> {
            assert_eq!(text, "warn");
            Ok(())
        }
        fn error(&self, text: String) -> Result<(), Self::Error> {
            assert_eq!(text, "error");
            Ok(())
        }
    }

    struct Numbergen;
    impl capability::Numbergen for Numbergen {
        type Error = Infallible;

        fn generate_guid(&self) -> Result<Uuid, Self::Error> {
            Ok(*UUID)
        }
        fn random_in_range(&self, min: u32, max: u32) -> Result<u32, Self::Error> {
            assert_eq!(min, 42);
            assert_eq!(max, 4242);
            Ok(42)
        }
        fn random_32(&self) -> Result<u32, Self::Error> {
            Ok(4242)
        }
    }

    #[derive(Deserialize)]
    struct HttpLogRngResponse {
        guid: String,
        random_in_range: u32,
        random_32: u32,
    }

    fn run_http_log_rng<'a>(caps: Option<impl IntoIterator<Item = &'a str>>) -> anyhow::Result<()> {
        _ = Lazy::force(&LOGGER);

        let claims = ClaimsBuilder::new()
            .issuer(&HTTP_LOG_RNG_MODULE.claims.issuer)
            .subject(&HTTP_LOG_RNG_MODULE.claims.subject)
            .with_metadata(jwt::Actor {
                name: Some("http_log_rng".into()),
                caps: caps.map(|caps| caps.into_iter().map(Into::into).collect()),
                ..jwt::Actor::default()
            })
            .build();
        let mut actor = HTTP_LOG_RNG_MODULE.clone();
        // Inject claims into actor directly to avoid (slow) recompilation of Wasm module
        actor.claims = claims;
        let mut actor = actor
            .instantiate(ActorInstanceConfig::default(), Logging, Numbergen, ())
            .expect("failed to instantiate actor");

        let ActorResponse {
            code,
            console_log,
            response,
        } = actor.call("HttpServer.HandleRequest", HTTP_LOG_RNG_REQUEST.as_slice())?;
        assert_eq!(code, 1);
        assert!(console_log.is_empty());

        let HttpResponse {
            status_code,
            header,
            body,
        } = deserialize(&response.expect("response missing"))
            .context("failed to deserialize response")?;
        assert_eq!(status_code, 200);
        assert!(header.is_empty());

        let HttpLogRngResponse {
            guid,
            random_in_range,
            random_32,
        } = serde_json::from_slice(&body).context("failed to decode body as JSON")?;
        assert_eq!(guid, UUID.to_string());
        assert_eq!(random_in_range, 42);
        assert_eq!(random_32, 4242);

        Ok(())
    }

    #[test]
    fn http_log_rng_valid() -> Result<()> {
        run_http_log_rng(Some([caps::LOGGING, caps::NUMBERGEN]))
    }

    #[test]
    fn http_log_rng_no_cap() {
        assert!(run_http_log_rng(Option::<[&'static str; 0]>::None).is_err());
    }

    #[test]
    fn http_log_rng_empty_cap() {
        assert!(run_http_log_rng(Some([])).is_err());
    }

    #[test]
    fn http_log_rng_no_numbergen_cap() {
        assert!(run_http_log_rng(Some([caps::LOGGING])).is_err());
    }

    #[test]
    fn http_log_rng_no_logging_cap() {
        assert!(run_http_log_rng(Some([caps::NUMBERGEN])).is_err());
    }
}
