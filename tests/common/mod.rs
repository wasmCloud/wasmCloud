use tokio::fs;
pub use wascap::{caps, jwt};
pub use wasmbus_rpc::common::{deserialize, serialize};

use std::path::Path;
use std::process::{Output, Stdio};

use anyhow::Context;
use once_cell::sync::Lazy;
use tempfile::NamedTempFile;
use tokio::process::Command;
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

/// Encode a component using `wasm-tools` as a library
pub fn encode_component_lib(module: &[u8], wasi: bool) -> anyhow::Result<Vec<u8>> {
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

/// Encode a component using `wasm-tools` as a binary
#[allow(dead_code)] // by some reason Rust fails to find usage of this function in parent crate
pub async fn encode_component_bin(path: impl AsRef<Path>, wasi: bool) -> anyhow::Result<Vec<u8>> {
    let wasm = NamedTempFile::new()
        .expect("failed to create temporary file")
        .into_temp_path();

    let mut cmd = Command::new(env!("CARGO_BIN_FILE_WASM_TOOLS"));
    let cmd = cmd
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .args(["component", "new"])
        .arg(path.as_ref())
        .arg("-o")
        .arg(wasm.as_os_str());
    if wasi {
        cmd.arg("--adapt").arg(format!(
            "wasi_snapshot_preview1={}",
            env!("CARGO_CDYLIB_FILE_WASI_SNAPSHOT_PREVIEW1")
        ));
    };
    let Output {
        status,
        stdout,
        stderr,
    } = cmd
        .spawn()
        .expect("failed to spawn `wasm-tools component`")
        .wait_with_output()
        .await
        .expect("failed to run `wasm-tools component`");
    eprintln!("stderr: {}", String::from_utf8(stderr).unwrap());
    assert!(
        status.success(),
        "stdout: {}",
        String::from_utf8(stdout).unwrap()
    );
    fs::read(wasm).await.context("failed to read Wasm")
}
