mod component;
mod module;

pub use component::{Component, ConfiguredComponent, Instance as ComponentInstance};
pub use module::{Config as ModuleConfig, ConfiguredModule, Instance as ModuleInstance, Module};

use crate::capability::{host, logging};
use crate::Runtime;

use core::fmt::Debug;

use anyhow::{ensure, Context, Result};
use futures::AsyncReadExt;
use tracing::instrument;
use wascap::jwt;
use wascap::wasm::extract_claims;

/// Extracts and validates claims contained within `WebAssembly` binary
fn claims(wasm: impl AsRef<[u8]>) -> Result<jwt::Claims<jwt::Actor>> {
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

    Ok(claims.claims)
}

/// A pre-loaded wasmCloud actor, which is either a module or a component
#[derive(Clone, Debug)]
pub enum Actor {
    /// WebAssembly module containing an actor
    Module(Module),
    /// WebAssembly component containing an actor
    Component(Component),
}

impl Actor {
    /// Compiles WebAssembly binary using [Runtime].
    ///
    /// # Errors
    ///
    /// Fails if [Component::new] or [Module::new] fails
    #[instrument(skip(wasm))]
    pub fn new(rt: &Runtime, wasm: impl AsRef<[u8]>) -> Result<Self> {
        let wasm = wasm.as_ref();
        // TODO: Optimize parsing, add functionality to `wascap` to parse from a custom section
        // directly
        match wasmparser::Parser::new(0).parse_all(wasm).next() {
            Some(Ok(wasmparser::Payload::Version {
                encoding: wasmparser::Encoding::Component,
                ..
            })) => Component::new(rt, wasm).map(Self::Component),
            // fallback to module type
            _ => Module::new(rt, wasm).map(Self::Module),
        }
    }

    /// Reads the WebAssembly binary asynchronously and calls [Actor::new].
    ///
    /// # Errors
    ///
    /// Fails if either reading `wasm` fails or [Self::new] fails
    #[instrument(skip(wasm))]
    pub async fn read(rt: &Runtime, mut wasm: impl futures::AsyncRead + Unpin) -> Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf)
            .await
            .context("failed to read Wasm")?;
        Self::new(rt, buf)
    }

    /// Reads the WebAssembly binary synchronously and calls [Actor::new].
    ///
    /// # Errors
    ///
    /// Fails if either reading `wasm` fails or [Self::new] fails
    #[instrument(skip(wasm))]
    pub fn read_sync(rt: &Runtime, mut wasm: impl std::io::Read) -> Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf).context("failed to read Wasm")?;
        Self::new(rt, buf)
    }

    /// [Claims](jwt::Claims) associated with this [Actor].
    #[instrument]
    pub fn claims(&self) -> &jwt::Claims<jwt::Actor> {
        match self {
            Self::Module(module) => module.claims(),
            Self::Component(component) => component.claims(),
        }
    }

    /// Instantiate the actor and invoke an operation on it.
    #[instrument]
    pub fn configure(&self) -> ConfiguredActor {
        self.into()
    }

    /// Like [Self::configure], but moves the [Actor].
    #[instrument]
    pub fn into_configure(self) -> ConfiguredActor {
        self.into()
    }

    /// Like [Self::configure], but moves the [Actor] and returns associated [jwt::Claims].
    #[instrument]
    pub fn into_configure_claims(self) -> (ConfiguredActor, jwt::Claims<jwt::Actor>) {
        self.into()
    }

    /// Instantiate the actor.
    ///
    /// # Errors
    ///
    /// Fails if [ConfiguredActor::instantiate] fails
    #[instrument]
    pub async fn instantiate(&self) -> anyhow::Result<Instance> {
        self.configure().instantiate().await
    }

    /// Instantiate the actor and invoke an operation on it.
    ///
    /// # Errors
    ///
    /// Fails if [ConfiguredActor::call] fails
    #[instrument(skip(operation, payload))]
    pub async fn call(
        &self,
        operation: impl AsRef<str>,
        payload: Option<impl Into<Vec<u8>> + AsRef<[u8]>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        self.configure().call(operation, payload).await
    }
}

impl From<Actor> for ConfiguredActor {
    fn from(actor: Actor) -> Self {
        match actor {
            Actor::Module(module) => Self::Module(module.into()),
            Actor::Component(component) => Self::Component(component.into()),
        }
    }
}

impl From<Actor> for (ConfiguredActor, jwt::Claims<jwt::Actor>) {
    fn from(actor: Actor) -> Self {
        match actor {
            Actor::Module(module) => {
                let (module, claims) = module.into_configure_claims();
                (ConfiguredActor::Module(module), claims)
            }
            Actor::Component(component) => {
                let (component, claims) = component.into_configure_claims();
                (ConfiguredActor::Component(component), claims)
            }
        }
    }
}

impl From<&Actor> for ConfiguredActor {
    fn from(actor: &Actor) -> Self {
        match actor {
            Actor::Module(module) => Self::Module(module.into()),
            Actor::Component(component) => Self::Component(component.into()),
        }
    }
}

/// A pre-loaded, configured wasmCloud actor, which is either a module or a component
#[derive(Debug)]
pub enum ConfiguredActor {
    /// WebAssembly module containing an actor
    Module(ConfiguredModule),
    /// WebAssembly component containing an actor
    Component(ConfiguredComponent),
}

impl ConfiguredActor {
    /// Set a [`host::Host`] handler to use for this instance
    #[must_use]
    pub fn host(self, host: impl host::Host + Sync + Send + 'static) -> Self {
        match self {
            Self::Module(module) => Self::Module(module.host(host)),
            Self::Component(component) => Self::Component(component.host(host)),
        }
    }

    /// Set a [`logging::Host`] handler to use for this instance
    #[must_use]
    pub fn logging(self, logging: impl logging::Host + Sync + Send + 'static) -> Self {
        match self {
            Self::Module(module) => Self::Module(module.logging(logging)),
            Self::Component(component) => Self::Component(component.logging(logging)),
        }
    }

    /// Configure actor to inherit standard output of the process
    #[must_use]
    pub fn inherit_stdout(self) -> Self {
        match self {
            Self::Module(module) => Self::Module(module.inherit_stdout()),
            Self::Component(component) => Self::Component(component.inherit_stdout()),
        }
    }

    /// Configure actor to inherit standard error of the process
    #[must_use]
    pub fn inherit_stderr(self) -> Self {
        match self {
            Self::Module(module) => Self::Module(module.inherit_stderr()),
            Self::Component(component) => Self::Component(component.inherit_stderr()),
        }
    }

    /// Instantiate the configured actor
    ///
    /// # Errors
    ///
    /// Fails if the underlying [Component::instantiate] or [Module::instantiate]
    #[instrument]
    pub async fn instantiate(self) -> anyhow::Result<Instance> {
        match self {
            Self::Module(module) => module.instantiate().await.map(Instance::Module),
            Self::Component(component) => component.instantiate().await.map(Instance::Component),
        }
    }

    /// Instantiate the configured actor and invoke an operation on it.
    ///
    /// # Errors
    ///
    /// Fails if the underlying [Component::call] or [Module::call]
    #[instrument(skip(operation, payload))]
    pub async fn call(
        self,
        operation: impl AsRef<str>,
        payload: Option<impl Into<Vec<u8>> + AsRef<[u8]>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        let operation = operation.as_ref();
        self.instantiate()
            .await
            .context("failed to instantiate actor")?
            .call(operation, payload)
            .await
            .with_context(|| format!("failed to call operation `{operation}` on actor"))
    }
}

/// A pre-loaded, configured wasmCloud actor instance, which is either a module or a component
#[derive(Debug)]
pub enum Instance {
    /// WebAssembly module containing an actor
    Module(ModuleInstance),
    /// WebAssembly component containing an actor
    Component(ComponentInstance),
}

impl Instance {
    /// Invoke an operation on an [Instance] producing a response
    ///
    /// # Errors
    ///
    /// Outermost error represents a failure in calling the actor, innermost - the
    /// application-layer error originating from within the actor itself
    #[instrument(skip_all)]
    pub async fn call(
        &mut self,
        operation: impl AsRef<str>,
        payload: Option<impl Into<Vec<u8>> + AsRef<[u8]>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        let operation = operation.as_ref();
        match self {
            Self::Module(module) => module
                .call(operation, payload.map(Into::into).unwrap_or(vec![]))
                .await
                .with_context(|| format!("failed to call operation `{operation}` on module")),
            Self::Component(component) => component
                .call(operation, payload)
                .await
                .with_context(|| format!("failed to call operation `{operation}` on component")),
        }
    }
}
