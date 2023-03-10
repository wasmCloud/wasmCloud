mod module;
mod wasmbus;

pub use module::{Instance as ModuleInstance, Module};

use core::fmt::{self, Debug};
use core::ptr::NonNull;

use std::sync::Arc;

use anyhow::{ensure, Context, Result};
use wascap::jwt;
use wascap::wasm::extract_claims;

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

struct Ctx<'a, H> {
    wasi: wasmtime_wasi::WasiCtx,
    wasmbus: wasmbus::Ctx<H>,
    claims: &'a jwt::Claims<jwt::Actor>,
}

impl<H> Debug for Ctx<'_, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("runtime", &"wasmtime")
            .field("wasmbus", &self.wasmbus)
            .field("claims", &self.claims)
            .finish()
    }
}

impl<'a, H> Ctx<'a, H> {
    fn new(claims: &'a jwt::Claims<jwt::Actor>, handler: Arc<H>) -> Result<Self> {
        // TODO: Set stdio pipes
        let wasi = wasmtime_wasi::WasiCtxBuilder::new()
            .arg("main.wasm")
            .context("failed to set argv[0]")?
            .build();
        let wasmbus = wasmbus::Ctx::new(handler);
        Ok(Self {
            wasi,
            wasmbus,
            claims,
        })
    }

    fn reset(&mut self) {
        self.wasmbus.reset();
    }
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

/// An actor [`ModuleInstance`] operation result returned in response to [`ModuleInstance::call`]
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
