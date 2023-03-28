#[cfg(feature = "component-model")]
mod component;
mod module;

#[cfg(feature = "component-model")]
pub use component::{Component, Instance as ComponentInstance};
use futures::AsyncReadExt;
pub use module::{
    Config as ModuleConfig, Instance as ModuleInstance, Module, Response as ModuleResponse,
};

use crate::Runtime;

use core::fmt::Debug;

use anyhow::{ensure, Context, Result};
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
#[derive(Clone)]
pub enum Actor {
    /// WebAssembly module containing an actor
    Module(Module),
    /// WebAssembly component containing an actor
    #[cfg(feature = "component-model")]
    Component(Component),
}

impl Debug for Actor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Module(module) => f.debug_tuple("Module").field(module).finish(),
            Self::Component(component) => f.debug_tuple("Component").field(component).finish(),
        }
    }
}

impl Actor {
    /// Compiles WebAssembly binary using [Runtime].
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
    #[instrument(skip(wasm))]
    pub async fn read(rt: &Runtime, mut wasm: impl futures::AsyncRead + Unpin) -> Result<Self> {
        let mut buf = Vec::new();
        wasm.read_to_end(&mut buf)
            .await
            .context("failed to read Wasm")?;
        Self::new(rt, buf)
    }

    /// Reads the WebAssembly binary synchronously and calls [Actor::new].
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
                .with_context(|| format!("failed to call operation `{operation}` on module")),
            Actor::Component(component) => component
                .call(operation, payload)
                .await
                .with_context(|| format!("failed to call operation `{operation}` on component")),
        }
    }

    /// Instantiate the actor and invoke an operation on it.
    /// The `call_context` argument is an opaque byte array that can be used for additional
    /// metadata around an actor call, like a parent span ID or invocation ID.
    #[instrument(skip(operation, payload, call_context))]
    pub async fn call_with_context(
        &self,
        operation: impl AsRef<str>,
        payload: Option<impl Into<Vec<u8>> + AsRef<[u8]>>,
        call_context: impl Into<Vec<u8>> + AsRef<[u8]>,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, String>> {
        let operation = operation.as_ref();
        match self {
            Actor::Module(module) => module
                .call_with_context(
                    operation,
                    payload.map(Into::into).unwrap_or(vec![]),
                    call_context,
                )
                .await
                .with_context(|| format!("failed to call operation `{operation}` on module")),
            Actor::Component(component) => component
                .call_with_context(operation, payload, call_context)
                .await
                .with_context(|| format!("failed to call operation `{operation}` on component")),
        }
    }
}
