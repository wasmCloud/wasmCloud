mod wasmbus;

use self::wasmbus::guest_call;

use crate::actor::claims;
use crate::capability::{Handle, Invocation};
use crate::Runtime;

use core::fmt::{self, Debug};

use std::sync::Arc;

use anyhow::{bail, ensure, Context, Result};
use tracing::{instrument, trace, warn};
use wascap::jwt;

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

pub(super) struct Ctx<'a> {
    pub wasi: wasmtime_wasi::WasiCtx,
    pub claims: &'a jwt::Claims<jwt::Actor>,
    pub wasmbus: wasmbus::Ctx,
}

impl Debug for Ctx<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("runtime", &"wasmtime")
            .field("wasmbus", &self.wasmbus)
            .field("claims", &self.claims)
            .finish()
    }
}

impl<'a> Ctx<'a> {
    fn new(
        claims: &'a jwt::Claims<jwt::Actor>,
        handler: Arc<Box<dyn Handle<Invocation>>>,
    ) -> Result<Self> {
        // TODO: Set stdio pipes
        let wasi = wasmtime_wasi::WasiCtxBuilder::new()
            .arg("main.wasm")
            .context("failed to set argv[0]")?
            .build();
        let wasmbus = wasmbus::Ctx::new(handler);
        Ok(Self {
            wasi,
            claims,
            wasmbus,
        })
    }

    fn reset(&mut self) {
        self.wasmbus.reset();
    }
}

/// Pre-compiled actor [Module], which is cheapily-[Cloneable](Clone)
pub struct Module {
    module: wasmtime::Module,
    claims: jwt::Claims<jwt::Actor>,
    handler: Arc<Box<dyn Handle<Invocation>>>,
    config: Config,
}

impl Clone for Module {
    fn clone(&self) -> Self {
        Self {
            module: self.module.clone(),
            claims: self.claims.clone(),
            handler: Arc::clone(&self.handler),
            config: self.config,
        }
    }
}

impl Debug for Module {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Module")
            .field("runtime", &"wasmtime")
            .field("claims", &self.claims)
            .field("config", &self.config)
            .finish()
    }
}

impl Module {
    /// [Claims](jwt::Claims) associated with this [Module].
    #[instrument]
    pub fn claims(&self) -> &jwt::Claims<jwt::Actor> {
        &self.claims
    }
}

impl Module {
    /// Extracts [Claims](jwt::Claims) from WebAssembly module and compiles it using [Runtime].
    #[instrument(skip(wasm))]
    pub fn new(rt: &Runtime, wasm: impl AsRef<[u8]>) -> Result<Self> {
        let wasm = wasm.as_ref();
        let claims = claims(wasm)?;
        let module = wasmtime::Module::new(&rt.engine, wasm).context("failed to compile module")?;
        Ok(Self {
            module,
            claims,
            handler: Arc::clone(&rt.handler),
            config: rt.module_config,
        })
    }

    /// Instantiates a [Module] and returns the resulting [Instance].
    #[instrument(skip_all)]
    pub async fn instantiate(&self) -> Result<Instance> {
        let engine = self.module.engine();

        let cx = Ctx::new(&self.claims, Arc::clone(&self.handler))
            .context("failed to construct store context")?;
        let mut store = wasmtime::Store::new(engine, cx);
        let mut linker = wasmtime::Linker::<Ctx>::new(engine);

        wasmtime_wasi::add_to_linker(&mut linker, |cx| &mut cx.wasi)
            .context("failed to link WASI")?;
        wasmbus::add_to_linker(&mut linker).context("failed to link wasmbus")?;

        let memory = wasmtime::Memory::new(
            &mut store,
            wasmtime::MemoryType::new(self.config.min_memory_pages, self.config.max_memory_pages),
        )
        .context("failed to initialize memory")?;
        linker
            .define_name(&store, "memory", memory)
            .context("failed to define `memory`")?;

        let instance = linker
            .instantiate_async(&mut store, &self.module)
            .await
            .context("failed to instantiate module")?;

        // TODO: call start etc.

        let func = instance
            .get_typed_func(&mut store, "__guest_call")
            .context("failed to get `__guest_call` export")?;
        Ok(Instance { func, store })
    }

    /// Instantiate a [Module] producing an [Instance] and invoke an operation on it using [Instance::call]
    #[instrument(skip(operation, payload))]
    pub async fn call(
        &self,
        operation: impl AsRef<str>,
        payload: impl Into<Vec<u8>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        let operation = operation.as_ref();
        let mut instance = self
            .instantiate()
            .await
            .context("failed to instantiate module")?;
        let Response {
            code,
            console_log,
            response,
        } = instance
            .call(operation, payload)
            .await
            .context("failed to call operation `{operation}` on module")?;
        ensure!(code == 1, "operation failed with exit code `{code}`");
        if !console_log.is_empty() {
            trace!(?console_log);
        }
        Ok(Ok(response))
    }
}

/// An actor module [`Instance`] operation result returned in response to [`Instance::call`]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Response {
    /// Code returned by an invocation of an operation on an actor [Instance].
    pub code: u32,
    /// Binary guest operation invocation response if returned by the guest.
    pub response: Option<Vec<u8>>,
    /// Console logs produced by a [Instance] operation invocation. Note, that this functionality
    /// is deprecated and should be empty in most cases.
    pub console_log: Vec<String>,
}

/// An instance of a [Module]
pub struct Instance<'a> {
    func: wasmtime::TypedFunc<guest_call::Params, guest_call::Result>,
    store: wasmtime::Store<Ctx<'a>>,
}

impl Instance<'_> {
    /// Invoke an operation on an [Instance] producing a [Response].
    #[instrument(skip_all)]
    pub async fn call(
        &mut self,
        operation: impl Into<String>,
        payload: impl Into<Vec<u8>>,
    ) -> Result<Response> {
        self.store.data_mut().reset();

        let operation = operation.into();
        let operation_len = operation
            .len()
            .try_into()
            .context("operation string length does not fit in u32")?;

        let payload = payload.into();
        let payload_len = payload
            .len()
            .try_into()
            .context("payload length does not fit in u32")?;

        self.store
            .data_mut()
            .wasmbus
            .set_guest_call(operation, payload);

        let code = self
            .func
            .call_async(&mut self.store, (operation_len, payload_len))
            .await
            .context("failed to call `__guest_call`")?;
        if let Some(err) = self.store.data_mut().wasmbus.take_guest_error() {
            bail!(err)
        } else if let Some(err) = self.store.data_mut().wasmbus.take_host_error() {
            bail!(err)
        }
        let response = self.store.data_mut().wasmbus.take_guest_response();
        let console_log = self.store.data_mut().wasmbus.take_console_log();
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

    use crate::capability::logging::Level;
    use crate::capability::numbergen::{self, Uuid};
    use crate::capability::{
        Handler, HandlerFunc, HostInvocation, LoggingInvocation, NumbergenInvocation,
    };

    use anyhow::Context;
    use async_trait::async_trait;
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
                        "info,wasmcloud=trace,cranelift_codegen=warn",
                    )
                }),
            )
            .init();
    });
    static UUID: Lazy<Uuid> = Lazy::new(Uuid::new_v4);

    struct Logging;

    #[async_trait]
    impl Handle<LoggingInvocation> for Logging {
        #[instrument(skip(self))]
        async fn handle(
            &self,
            _: &jwt::Claims<jwt::Actor>,
            _: String,
            invocation: LoggingInvocation,
        ) -> Result<Option<Vec<u8>>> {
            match invocation {
                LoggingInvocation::WriteLog {
                    level: Level::Debug,
                    text,
                } => {
                    assert_eq!(text, "debug");
                    Ok(None)
                }
                LoggingInvocation::WriteLog {
                    level: Level::Info,
                    text,
                } => {
                    assert_eq!(text, "info");
                    Ok(None)
                }
                LoggingInvocation::WriteLog {
                    level: Level::Warn,
                    text,
                } => {
                    assert_eq!(text, "warn");
                    Ok(None)
                }
                LoggingInvocation::WriteLog {
                    level: Level::Error,
                    text,
                } => {
                    assert_eq!(text, "error");
                    Ok(None)
                }
            }
        }
    }

    struct Numbergen;

    #[async_trait]
    impl Handle<NumbergenInvocation> for Numbergen {
        #[instrument(skip(self))]
        async fn handle(
            &self,
            _: &jwt::Claims<jwt::Actor>,
            _: String,
            invocation: NumbergenInvocation,
        ) -> Result<Option<Vec<u8>>> {
            match invocation {
                NumbergenInvocation::GenerateGuid => {
                    let guid = numbergen::serialize_response(&UUID.to_string())
                        .expect("failed to serialize response");
                    Ok(Some(guid))
                }
                NumbergenInvocation::RandomInRange { min, max } => {
                    assert_eq!(min, 42);
                    assert_eq!(max, 4242);
                    let v = numbergen::serialize_response(&42u32)
                        .expect("failed to serialize response");
                    Ok(Some(v))
                }
                NumbergenInvocation::Random32 => {
                    let v = numbergen::serialize_response(&4242u32)
                        .expect("failed to serialize response");
                    Ok(Some(v))
                }
            }
        }
    }

    #[allow(clippy::unused_async)]
    #[instrument]
    async fn host_call(
        claims: jwt::Claims<jwt::Actor>,
        binding: String,
        invocation: HostInvocation,
    ) -> anyhow::Result<Option<[u8; 0]>> {
        panic!(
            "cannot execute `{invocation:?}` within binding `{binding}` for actor `{}`",
            claims.subject
        )
    }

    static RUNTIME: Lazy<Runtime> = Lazy::new(|| {
        Runtime::new(Handler {
            logging: Logging,
            numbergen: Numbergen,
            host: HandlerFunc::from(host_call),
        })
        .expect("failed to construct runtime")
    });

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
        const HTTP_LOG_RNG_WASM: &str = env!("CARGO_CDYLIB_FILE_ACTOR_HTTP_LOG_RNG_MODULE");
        let wasm = std::fs::read(HTTP_LOG_RNG_WASM)
            .unwrap_or_else(|_| panic!("failed to read `{HTTP_LOG_RNG_WASM}`"));

        let issuer = KeyPair::new_account();
        let module = KeyPair::new_module();

        let claims = ClaimsBuilder::new()
            .issuer(&issuer.public_key())
            .subject(&module.public_key())
            .with_metadata(jwt::Actor::default()) // this will be overriden by individual test cases
            .build();
        let wasm = embed_claims(&wasm, &claims, &issuer).expect("failed to embed actor claims");

        let actor = Module::new(&RUNTIME, wasm.as_slice()).expect("failed to read actor module");
        assert_eq!(actor.claims().subject, module.public_key());

        actor
    });

    #[derive(Deserialize)]
    struct HttpLogRngResponse {
        guid: String,
        random_in_range: u32,
        random_32: u32,
    }

    async fn run_http_log_rng<'a>(
        caps: Option<impl IntoIterator<Item = &'a str>>,
    ) -> anyhow::Result<()> {
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
            .instantiate()
            .await
            .expect("failed to instantiate actor");

        let Response {
            code,
            console_log,
            response,
        } = actor
            .call("HttpServer.HandleRequest", HTTP_LOG_RNG_REQUEST.as_slice())
            .await?;
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

    #[tokio::test]
    async fn http_log_rng() -> Result<()> {
        run_http_log_rng(Some([caps::LOGGING, caps::NUMBERGEN])).await
    }
}
