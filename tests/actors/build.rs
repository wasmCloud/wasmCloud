use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use nkeys::KeyPair;
use wascap::caps;
use wasmcloud_component_adapters::{
    WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER, WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER,
};

use anyhow::{bail, ensure, Context};
use futures::try_join;
use serde::Deserialize;
use tokio::fs;
use tokio::process::Command;

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
) -> anyhow::Result<impl Iterator<Item = (String, Vec<PathBuf>)>> {
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

async fn copy(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> anyhow::Result<u64> {
    let src = src.as_ref();
    let dst = dst.as_ref();
    fs::copy(&src, &dst)
        .await
        .with_context(|| format!("failed to copy `{}` to `{}`", src.display(), dst.display()))
}

trait DerefArtifact {
    fn deref_artifact(&self) -> Option<(&str, &[PathBuf])>;
}

impl DerefArtifact for Option<(String, Vec<PathBuf>)> {
    fn deref_artifact(&self) -> Option<(&str, &[PathBuf])> {
        self.as_ref()
            .map(|(pkg, paths)| (pkg.as_str(), paths.as_slice()))
    }
}

async fn install_rust_wasm32_unknown_unknown_actors(
    out_dir: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let out_dir = out_dir.as_ref();
    let mut artifacts = build_artifacts(
        [
            "--manifest-path=./rust/Cargo.toml",
            "--target=wasm32-unknown-unknown",
            "-p=builtins-module-reactor",
            "-p=kv-http-smithy",
            "-p=blobstore-http-smithy",
        ],
        |name, kind| {
            [
                "blobstore-http-smithy",
                "builtins-module-reactor",
                "kv-http-smithy",
            ]
            .contains(&name)
                && kind.contains(&CrateType::Cdylib)
        },
    )
    .await
    .context("failed to build `builtins-module-reactor` crate")?;
    match (
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next(),
    ) {
        (
            // NOTE: this list of artifacts must stay sorted
            Some(("blobstore-http-smithy", [blobstore_http_smithy])),
            Some(("builtins-module-reactor", [builtins_module_reactor])),
            Some(("kv-http-smithy", [kv_http_smithy])),
            None,
        ) => {
            copy(
                builtins_module_reactor,
                out_dir.join("rust-builtins-module-reactor.wasm"),
            )
            .await?;
            copy(kv_http_smithy, out_dir.join("rust-kv-http-smithy.wasm")).await?;
            copy(
                blobstore_http_smithy,
                out_dir.join("rust-blobstore-http-smithy.wasm"),
            )
            .await?;
            Ok(())
        }
        _ => bail!("invalid `builtins-module-reactor` build artifacts"),
    }
}

async fn install_rust_wasm32_wasi_actors(out_dir: impl AsRef<Path>) -> anyhow::Result<()> {
    let out_dir = out_dir.as_ref();

    try_join!(
        // Build component actors
        async {
            let mut artifacts = build_artifacts(
                [
                    "--manifest-path=./rust/Cargo.toml",
                    "--target=wasm32-wasi",
                    "-p=builtins-component-reactor",
                    "-p=foobar-component-command",
                ],
                |name, kind| {
                    ["builtins-component-reactor", "foobar-component-command"].contains(&name)
                        && (kind.contains(&CrateType::Cdylib) || kind.contains(&CrateType::Bin))
                },
            )
            .await
            .context("failed to build `builtins-component-reactor` and `foobar-component-command` crates")?;
            match (
                artifacts.next().deref_artifact(),
                artifacts.next().deref_artifact(),
                artifacts.next()
            ) {
                (
                    Some(("builtins-component-reactor", [builtins_component_reactor])),
                    Some(("foobar-component-command", [foobar_component_command])),
                    None
                ) => {
                    try_join!(
                        copy(
                            builtins_component_reactor,
                            out_dir.join("rust-builtins-component-reactor.wasm"),
                        ),
                        copy(
                            foobar_component_command,
                            out_dir.join("rust-foobar-component-command.wasm"),
                        )
                    )
                }
                _ => bail!("invalid `builtins-component-reactor` and `foobar-component-command` build artifacts"),
            }
        },

        // Build non-component (module) actors
        async {
            let mut artifacts = build_artifacts(
                [
                    "--manifest-path=./rust/Cargo.toml",
                    "--target=wasm32-wasi",
                    "-p=logging-module-command",
                ],
                |name, kind| {
                    ["logging-module-command"].contains(&name) && kind.contains(&CrateType::Bin)
                },
            )
            .await
            .context("failed to build `logging-module-command` crate")?;
            match (artifacts.next().deref_artifact(), artifacts.next()) {
                (Some(("logging-module-command", [logging_module_command])), None) => {
                    copy(
                        logging_module_command,
                        out_dir.join("rust-logging-module-command.wasm"),
                    )
                    .await
                }
                _ => bail!("invalid `logging-module-command` build artifacts"),
            }
        },
    )
    .context("failed to build `wasm32-wasi` actors")?;
    Ok(())
}

#[cfg(not(feature = "docs"))]
fn encode_component(module: impl AsRef<[u8]>, adapter: &[u8]) -> anyhow::Result<Vec<u8>> {
    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(module.as_ref())
        .context("failed to set core component module")?
        .adapter("wasi_snapshot_preview1", adapter)
        .context("failed to add WASI adapter")?
        .encode()
        .context("failed to encode a component")
}

#[cfg(feature = "docs")]
fn encode_component(_: impl AsRef<[u8]>, _: &[u8]) -> anyhow::Result<Vec<u8>> {
    Ok(Vec::default())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=../../crates/actor");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=rust");

    let out_dir = env::var("OUT_DIR")
        .map(PathBuf::from)
        .context("failed to lookup `OUT_DIR`")?;

    // Build both traditional wasm32-unknown-unknown and wasm32-wasi actors
    try_join!(
        install_rust_wasm32_unknown_unknown_actors(&out_dir),
        install_rust_wasm32_wasi_actors(&out_dir),
    )?;

    // Build WASI component wasm modules
    for name in ["builtins-component-reactor"] {
        let path = out_dir.join(format!("rust-{name}.wasm"));
        let module = fs::read(&path)
            .await
            .with_context(|| format!("failed to read `{}`", path.display()))?;
        let component = encode_component(module, WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER)
            .with_context(|| format!("failed to encode `{}`", path.display()))?;

        let path = out_dir.join(format!("rust-{name}-preview2.wasm"));
        fs::write(&path, component)
            .await
            .with_context(|| format!("failed to write `{}`", path.display()))?;
    }
    for name in ["foobar-component-command"] {
        let path = out_dir.join(format!("rust-{name}.wasm"));
        let module = fs::read(&path)
            .await
            .with_context(|| format!("failed to read `{}`", path.display()))?;
        let component = encode_component(module, WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER)
            .with_context(|| format!("failed to encode `{}`", path.display()))?;

        let path = out_dir.join(format!("rust-{name}-preview2.wasm"));
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

    // Sign the built wasm modules with relevant claims
    let builtin_caps: Vec<String> = vec![
        caps::BLOB.into(),
        caps::HTTP_CLIENT.into(),
        caps::HTTP_SERVER.into(),
        caps::KEY_VALUE.into(),
        caps::LOGGING.into(),
        caps::MESSAGING.into(),
        caps::NUMBERGEN.into(),
    ];
    for (name, caps) in [
        ("builtins-component-reactor", Some(builtin_caps.clone())),
        (
            "builtins-component-reactor-preview2",
            Some(builtin_caps.clone()),
        ),
        ("builtins-module-reactor", Some(builtin_caps.clone())),
        ("foobar-component-command", None),
        ("foobar-component-command-preview2", None),
        ("logging-module-command", Some(vec![caps::LOGGING.into()])),
        (
            "kv-http-smithy",
            Some(vec![caps::HTTP_SERVER.into(), caps::KEY_VALUE.into()]),
        ),
        (
            "blobstore-http-smithy",
            Some(vec![caps::HTTP_SERVER.into(), caps::BLOB.into()]),
        ),
    ] {
        let wasm = fs::read(out_dir.join(format!("rust-{name}.wasm")))
            .await
            .with_context(|| format!("failed to read `{name}` Wasm"))?;
        let wasm = if cfg!(feature = "docs") {
            _ = caps;
            wasm
        } else {
            let module = KeyPair::new_module();
            let claims = wascap::prelude::ClaimsBuilder::new()
                .issuer(&issuer.public_key())
                .subject(&module.public_key())
                .with_metadata(wascap::jwt::Actor {
                    name: Some(name.into()),
                    caps,
                    call_alias: Some(name.into()),
                    ..Default::default()
                })
                .build();
            wascap::wasm::embed_claims(&wasm, &claims, &issuer)
                .context("failed to embed actor claims")?
        };
        fs::write(out_dir.join(format!("rust-{name}.signed.wasm")), wasm)
            .await
            .context("failed to write Wasm")?;
    }

    Ok(())
}
