mod component;

pub use component::{
    Component, Instance as ComponentInstance, InterfaceInstance as ComponentInterfaceInstance,
};

use core::fmt::Debug;

use anyhow::{ensure, Context, Result};
use wascap::jwt;
use wascap::wasm::extract_claims;

/// Component instance configuration
#[derive(Clone, Debug, Default)]
pub struct Config {
    /// Whether actors are required to be signed to be executed
    pub require_signature: bool,
}

/// Extracts and validates claims contained within a WebAssembly binary, if present
///
/// # Arguments
///
/// * `wasm` - Bytes that constitute a valid WebAssembly binary
fn claims(wasm: impl AsRef<[u8]>) -> Result<Option<jwt::Claims<jwt::Component>>> {
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
    Ok(Some(claims.claims))
}
