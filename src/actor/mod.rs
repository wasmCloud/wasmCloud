#[cfg(feature = "component-model")]
mod component;
mod module;

#[cfg(feature = "component-model")]
pub use component::{Component, Instance as ComponentInstance};
pub use module::{
    Config as ModuleConfig, Instance as ModuleInstance, Module, Response as ModuleResponse,
};

use anyhow::{ensure, Context, Result};
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
