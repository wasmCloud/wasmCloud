#[cfg(feature = "component-model")]
mod component;
mod module;

#[cfg(feature = "component-model")]
pub use component::{Component, Instance as ComponentInstance};
use futures::AsyncReadExt;
pub use module::{
    Config as ModuleConfig, Instance as ModuleInstance, Module, Response as ModuleResponse,
};

use crate::{capability, Runtime};

use core::fmt::Debug;

use anyhow::{ensure, Context, Result};
use tracing::instrument;
use wascap::jwt;
use wascap::wasm::extract_claims;

/// Extracts and validates claims contained within `WebAssembly` module
fn actor_claims(wasm: impl AsRef<[u8]>) -> Result<jwt::Claims<jwt::Actor>> {
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
#[derive(Clone)]
pub enum Actor<H = Box<dyn capability::Handler<Error = String>>> {
    /// WebAssembly module containing an actor
    Module(Module<H>),
    /// WebAssembly component containing an actor
    #[cfg(feature = "component-model")]
    Component(Component<H>),
}

impl<H> Debug for Actor<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Module(module) => f.debug_tuple("Module").field(module).finish(),
            Self::Component(component) => f.debug_tuple("Component").field(component).finish(),
        }
    }
}

impl<H: capability::Handler + 'static> Actor<H> {
    /// Compiles WebAssembly binary using [Runtime].
    #[instrument(skip(wasm))]
    pub fn new(rt: &Runtime<H>, wasm: impl AsRef<[u8]>) -> Result<Self> {
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
    #[instrument(skip(wasm))]
    pub async fn read(rt: &Runtime<H>, mut wasm: impl futures::AsyncRead + Unpin) -> Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf)
            .await
            .context("failed to read Wasm")?;
        Self::new(rt, buf)
    }

    /// Reads the WebAssembly binary synchronously and calls [Actor::new].
    #[instrument(skip(wasm))]
    pub fn read_sync(rt: &Runtime<H>, mut wasm: impl std::io::Read) -> Result<Self> {
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
    #[instrument(skip(operation, payload))]
    pub async fn call(
        &self,
        operation: impl AsRef<str>,
        payload: Option<impl Into<Vec<u8>> + AsRef<[u8]>>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        let operation = operation.as_ref();
        match self {
            Actor::Module(module) => module
                .call(operation, payload.map(Into::into).unwrap_or(vec![]))
                .await
                .context("failed to call operation `{operation}` on module"),
            Actor::Component(component) => component
                .call(operation, payload)
                .await
                .context("failed to call operation `{operation}` on component"),
        }
    }
}
