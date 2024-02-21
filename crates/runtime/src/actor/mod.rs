mod component;

pub use component::{
    Component, Instance as ComponentInstance, InterfaceInstance as ComponentInterfaceInstance,
};

use core::fmt::Debug;

use anyhow::{ensure, Context};
use wascap::jwt;
use wascap::wasm::extract_claims;

/// Actor instance configuration
#[derive(Clone, Debug, Default)]
pub struct Config {
    /// Whether actors are required to be signed to be executed
    pub require_signature: bool,
}

/// Extracts and validates claims contained within `WebAssembly` binary, if such are found
fn claims(wasm: impl AsRef<[u8]>) -> anyhow::Result<Option<jwt::Claims<jwt::Actor>>> {
    let Some(claims) = extract_claims(wasm).context("failed to extract module claims")? else {
        return Ok(None);
    };
    let v = jwt::validate_token::<jwt::Actor>(&claims.jwt)
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
