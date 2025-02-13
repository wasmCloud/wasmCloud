use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Arc;

use anyhow::{bail, ensure, Context, Result};
use heck::ToKebabCase;
use nkeys::KeyPair;
use serde::Deserialize;
use tokio::fs;
use tokio::process::Command;
use tokio::task::JoinSet;
use wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER;

/// List of (manifest path, output artifact name) for all the packages used during test
///
/// Manifest paths should be relative to the directory containing this build.rs (i.e. tests/components)
const WASI_WASM32_PACKAGES: [(&str, &str); 7] = [
    ("./rust/Cargo.toml", "interfaces-handler-reactor"),
    ("./rust/Cargo.toml", "interfaces-reactor"),
    ("./rust/Cargo.toml", "pinger-config-component"),
    ("./rust/Cargo.toml", "ponger-config-component"),
    (
        "../../examples/rust/components/http-hello-world/Cargo.toml",
        "http-hello-world",
    ),
    (
        "../../examples/rust/components/http-keyvalue-counter/Cargo.toml",
        "http-keyvalue-counter",
    ),
    (
        "../../examples/rust/components/http-keyvalue-watcher/Cargo.toml",
        "http-keyvalue-watcher",
    ),
];

/// List of packages which should have output artifacts signed
const WASI_WASM32_PACKAGES_SIGNED: [&str; 10] = [
    "http-keyvalue-counter",
    "http-keyvalue-counter-preview2",
    "interfaces-handler-reactor",
    "interfaces-handler-reactor-preview2",
    "interfaces-reactor",
    "interfaces-reactor-preview2",
    "pinger-config-component",
    "pinger-config-component-preview2",
    "ponger-config-component",
    "ponger-config-component-preview2",
];

/// Convert a hard-coded artifact name (see [`WASI_WASM32_PACKAGES`])
///
/// Pre-Rust 1.79, kebab cased project names would turn into kebab-cased artifacts
#[rustversion::before(1.79)]
fn generate_expected_artifact_name(project_name: &str) -> impl AsRef<str> {
    project_name.to_kebab_case()
}

/// Convert a hard-coded artifact name (see [`WASI_WASM32_PACKAGES`])
///
/// Post-Rust 1.79, kebab cased project names turn into snake_case cased artifacts
#[rustversion::since(1.79)]
fn generate_expected_artifact_name(project_name: &str) -> impl AsRef<str> {
    use heck::ToSnakeCase;
    project_name.to_snake_case()
}

// Unfortunately, `cargo` exported structs and enums do not implement `Deserialize`, so
// implement the relevant parts here

//https://github.com/rust-lang/cargo/blob/b0742b2145f02d3557f596d1ee4b36c0426f39ab/src/cargo/core/compiler/crate_type.rs#L8-L17
#[derive(Deserialize, Eq, PartialEq)]
enum CrateType {
    #[serde(rename = "bin")]
    Bin,
    #[serde(rename = "lib")]
    Lib,
    #[serde(rename = "rlib")]
    Rlib,
    #[serde(rename = "dylib")]
    Dylib,
    #[serde(rename = "cdylib")]
    Cdylib,
    #[serde(rename = "staticlib")]
    Staticlib,
    #[serde(rename = "procmacro")]
    ProcMacro,
    #[serde(other)]
    Other,
}

// from https://github.com/rust-lang/cargo/blob/b0742b2145f02d3557f596d1ee4b36c0426f39ab/src/cargo/core/manifest.rs#L267-L286
#[derive(Deserialize)]
struct Target {
    name: String,
    kind: Vec<CrateType>,
}

#[derive(Deserialize)]
#[serde(tag = "reason")]
enum BuildMessage {
    // from https://github.com/rust-lang/cargo/blob/b0742b2145f02d3557f596d1ee4b36c0426f39ab/src/cargo/util/machine_message.rs#L34-L44
    #[serde(rename = "compiler-artifact")]
    CompilerArtifact {
        target: Target,
        filenames: Vec<PathBuf>,
    },
    #[serde(other)]
    Other,
}

async fn build_artifacts(
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    pred: impl Fn(&str, &[CrateType]) -> bool,
) -> Result<impl Iterator<Item = (String, Vec<PathBuf>)>> {
    let Output {
        status,
        stdout,
        stderr: _, // inherited
    } = Command::new(env::var("CARGO").unwrap())
        .env("CARGO_ENCODED_RUSTFLAGS", "--cfg\x1ftokio_unstable") // Enable tokio on WASI
        .args(["build", "--message-format=json-render-diagnostics"])
        .args(args)
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn `cargo` process")?
        .wait_with_output()
        .await
        .context("failed to call `cargo`")?;
    ensure!(status.success(), "`cargo` invocation failed");

    serde_json::Deserializer::from_reader(stdout.as_slice())
        .into_iter()
        .filter_map(|message| match message {
            Ok(BuildMessage::CompilerArtifact {
                target: Target { name, kind },
                filenames,
            }) if pred(&name, &kind) => Some((name, filenames)),
            _ => None,
        })
        .try_fold(BTreeMap::new(), |mut artifacts, (pkg, files)| {
            use std::collections::btree_map::Entry::{Occupied, Vacant};

            match artifacts.entry(pkg) {
                Occupied(e) => bail!("duplicate entry for `{}`", e.key()),
                Vacant(e) => {
                    e.insert(files);
                    Ok(artifacts)
                }
            }
        })
        .map(IntoIterator::into_iter)
}

/// Copy a file from `src` to `dst`
async fn copy(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<u64> {
    let src = src.as_ref();
    let dst = dst.as_ref();
    fs::copy(&src, &dst)
        .await
        .with_context(|| format!("failed to copy `{}` to `{}`", src.display(), dst.display()))
}

async fn install_rust_wasm32_wasi_actors(out_dir: impl AsRef<Path>) -> Result<()> {
    let out_dir = Arc::new(out_dir.as_ref().to_path_buf());

    // Build the artifacts in parallel
    let mut artifacts = JoinSet::new();
    for (manifest_path, package) in WASI_WASM32_PACKAGES {
        artifacts.spawn(build_artifacts(
            [
                format!(
                    "--manifest-path={}/{manifest_path}",
                    env!("CARGO_MANIFEST_DIR")
                ),
                "--target=wasm32-wasip1".to_string(),
                // We use dashes for packages, since they're still always
                format!("-p={package}"),
            ],
            |name, kind| {
                WASI_WASM32_PACKAGES
                    .iter()
                    .any(|(_, n)| generate_expected_artifact_name(n).as_ref() == name)
                    && (kind.contains(&CrateType::Cdylib) || kind.contains(&CrateType::Bin))
            },
        ));
    }

    // Copy the artifacts to expected paths,
    let mut copies = JoinSet::new();
    while let Some(Ok(Ok(mut iter))) = artifacts.join_next().await {
        let out_dir = out_dir.clone();
        if let Some((artifact, paths)) = iter.next() {
            copies.spawn(async move {
                copy(
                    paths
                        .first()
                        .context("failed to get back path from artifact build")?,
                    // Code still expects kebab case (according to project names, not artifact names)
                    // This is a no-op for pre-1.79, but is required 1.79 or after
                    out_dir.join(format!("rust-{}.wasm", artifact.to_kebab_case())),
                )
                .await
            });
        }
    }

    // Wait for all the copies
    while (copies.join_next().await).is_some() {}

    Ok(())
}

#[cfg(not(feature = "docs"))]
fn encode_component(module: impl AsRef<[u8]>, adapter: &[u8]) -> Result<Vec<u8>> {
    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(module.as_ref())
        .context("failed to set core component module")?
        .adapter(
            wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_ADAPTER_NAME,
            adapter,
        )
        .context("failed to add WASI adapter")?
        .encode()
        .context("failed to encode a component")
}

#[cfg(feature = "docs")]
fn encode_component(_: impl AsRef<[u8]>, _: &[u8]) -> Result<Vec<u8>> {
    Ok(Vec::default())
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=../../crates/component");
    println!("cargo:rerun-if-changed=rust");

    let out_dir = env::var("OUT_DIR")
        .map(PathBuf::from)
        .context("failed to lookup `OUT_DIR`")?;
    install_rust_wasm32_wasi_actors(&out_dir).await?;

    // Build WASI reactor components
    for (_manifest_path, package_name) in WASI_WASM32_PACKAGES {
        let path = out_dir.join(format!("rust-{package_name}.wasm"));
        let module = fs::read(&path)
            .await
            .with_context(|| format!("failed to read `{}`", path.display()))?;
        let component = encode_component(module, WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER)
            .with_context(|| format!("failed to encode `{}`", path.display()))?;
        let path = out_dir.join(format!("rust-{package_name}-preview2.wasm"));
        fs::write(&path, component)
            .await
            .with_context(|| format!("failed to write `{}`", path.display()))?;
    }

    // Create a new keypair to use when signing wasm modules built for test
    let issuer = KeyPair::new_account();
    println!(
        "cargo:rustc-env=ISSUER={}",
        issuer.seed().expect("failed to extract issuer seed")
    );

    for package_name in WASI_WASM32_PACKAGES_SIGNED {
        let wasm = fs::read(out_dir.join(format!("rust-{package_name}.wasm")))
            .await
            .with_context(|| format!("failed to read `{package_name}` Wasm"))?;
        let wasm = if cfg!(feature = "docs") {
            wasm
        } else {
            let module = KeyPair::new_module();
            let claims = wascap::prelude::ClaimsBuilder::new()
                .issuer(&issuer.public_key())
                .subject(&module.public_key())
                .with_metadata(wascap::jwt::Component {
                    name: Some(package_name.into()),
                    call_alias: Some(package_name.into()),
                    ..Default::default()
                })
                .build();
            wascap::wasm::embed_claims(&wasm, &claims, &issuer)
                .context("failed to embed component claims")?
        };
        fs::write(
            out_dir.join(format!("rust-{package_name}.signed.wasm")),
            wasm,
        )
        .await
        .context("failed to write Wasm")?;
    }

    Ok(())
}
