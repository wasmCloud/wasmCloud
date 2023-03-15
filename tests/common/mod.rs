pub use wascap::{caps, jwt};
pub use wasmbus_rpc::common::{deserialize, serialize};

use anyhow::Context;
use once_cell::sync::Lazy;
use tracing_subscriber::prelude::*;
use wascap::prelude::{ClaimsBuilder, KeyPair};
use wascap::wasm::embed_claims;
use wit_component::ComponentEncoder;

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

pub fn init() {
    _ = Lazy::force(&LOGGER);
}

pub fn sign(
    wasm: impl AsRef<[u8]>,
    name: impl Into<String>,
    caps: impl IntoIterator<Item = &'static str>,
) -> anyhow::Result<(Vec<u8>, KeyPair)> {
    let issuer = KeyPair::new_account();
    let module = KeyPair::new_module();

    let claims = ClaimsBuilder::new()
        .issuer(&issuer.public_key())
        .subject(&module.public_key())
        .with_metadata(jwt::Actor {
            name: Some(name.into()),
            caps: Some(caps.into_iter().map(Into::into).collect()),
            ..Default::default()
        })
        .build();
    let wasm =
        embed_claims(wasm.as_ref(), &claims, &issuer).context("failed to embed actor claims")?;
    Ok((wasm, module))
}

pub fn encode_component(module: &[u8], wasi: bool) -> anyhow::Result<Vec<u8>> {
    let encoder = ComponentEncoder::default()
        .validate(true)
        .module(module)
        .context("failed to set core component module")?;
    let encoder = if wasi {
        encoder
            .adapter(
                "wasi_snapshot_preview1",
                include_bytes!(env!("CARGO_CDYLIB_FILE_WASI_SNAPSHOT_PREVIEW1")),
            )
            .context("failed to add WASI adapter")?
    } else {
        encoder
    };
    encoder.encode().context("failed to encode a component")
}
