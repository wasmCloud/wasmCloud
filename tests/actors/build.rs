use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use nkeys::KeyPair;
use wascap::prelude::ClaimsBuilder;
use wascap::wasm::embed_claims;
use wascap::{caps, jwt};
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
        ],
        |name, kind| {
            ["builtins-module-reactor"].contains(&name) && kind.contains(&CrateType::Cdylib)
        },
    )
    .await
    .context("failed to build `builtins-module-reactor` crate")?;
    match (artifacts.next().deref_artifact(), artifacts.next()) {
        (Some(("builtins-module-reactor", [builtins_module_reactor])), None) => {
            copy(
                builtins_module_reactor,
                out_dir.join("rust-builtins-module-reactor.wasm"),
            )
            .await?;
            Ok(())
        }
        _ => bail!("invalid `builtins-module-reactor` build artifacts"),
    }
}

async fn install_rust_wasm32_wasi_actors(out_dir: impl AsRef<Path>) -> anyhow::Result<()> {
    let out_dir = out_dir.as_ref();

    // NOTE: Due to bizarre nature of `cargo` feature unification, compiling builtins actors in a
    // singular `cargo` invocation would unify `component` and `compat` features in
    // `wasmcloud_actor` crate

    try_join!(
        async {
            let mut artifacts = build_artifacts(
                [
                    "--manifest-path=./rust/Cargo.toml",
                    "--target=wasm32-wasi",
                    "-p=builtins-compat-reactor",
                    "-p=http-compat-command",
                ],
                |name, kind| {
                    ["builtins-compat-reactor", "http-compat-command"].contains(&name)
                        && (kind.contains(&CrateType::Cdylib) || kind.contains(&CrateType::Bin))
                },
            )
            .await
            .context(
                "failed to build `builtins-compat-reactor` and `http-compat-command` crates",
            )?;
            match (
                artifacts.next().deref_artifact(),
                artifacts.next().deref_artifact(),
                artifacts.next(),
            ) {
                (
                    Some(("builtins-compat-reactor", [builtins_compat_reactor])),
                    Some(("http-compat-command", [http_compat_command])),
                    None,
                ) => {
                    try_join!(
                        copy(
                            builtins_compat_reactor,
                            out_dir.join("rust-builtins-compat-reactor.wasm"),
                        ),
                        copy(
                            http_compat_command,
                            out_dir.join("rust-http-compat-command.wasm"),
                        ),
                    )
                }
                _ => bail!(
                    "invalid `builtins-compat-reactor` and `http-compat-command` build artifacts"
                ),
            }
        },
        async {
            let mut artifacts = build_artifacts(
                [
                    "--manifest-path=./rust/Cargo.toml",
                    "--target=wasm32-wasi",
                    "-p=builtins-component-reactor",
                ],
                |name, kind| {
                    ["builtins-component-reactor"].contains(&name)
                        && kind.contains(&CrateType::Cdylib)
                },
            )
            .await
            .context("failed to build `builtins-component-reactor` crate")?;
            match (artifacts.next().deref_artifact(), artifacts.next()) {
                (Some(("builtins-component-reactor", [builtins_component_reactor])), None) => {
                    copy(
                        builtins_component_reactor,
                        out_dir.join("rust-builtins-component-reactor.wasm"),
                    )
                    .await
                }
                _ => bail!("invalid `builtins-component-reactor` build artifacts"),
            }
        },
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
        async {
            let mut artifacts = build_artifacts(
                [
                    "--manifest-path=./rust/tcp-component-command/Cargo.toml",
                    "--target=wasm32-wasi",
                ],
                |name, kind| {
                    ["tcp-component-command"].contains(&name) && kind.contains(&CrateType::Bin)
                },
            )
            .await
            .context("failed to build `tcp-component-command` crate")?;
            match (artifacts.next().deref_artifact(), artifacts.next()) {
                (Some(("tcp-component-command", [tcp_component_command])), None) => {
                    copy(
                        tcp_component_command,
                        out_dir.join("rust-tcp-component-command.wasm"),
                    )
                    .await
                }
                _ => bail!("invalid `tcp-component-command` build artifacts"),
            }
        }
    )
    .context("failed to build `wasm32-wasi` actors")?;
    Ok(())
}

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=rust");

    let out_dir = env::var("OUT_DIR")
        .map(PathBuf::from)
        .context("failed to lookup `OUT_DIR`")?;
    try_join!(
        install_rust_wasm32_unknown_unknown_actors(&out_dir),
        install_rust_wasm32_wasi_actors(&out_dir),
    )?;
    for name in ["builtins-compat-reactor", "builtins-component-reactor"] {
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
    for name in ["http-compat-command", "tcp-component-command"] {
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

    let issuer = KeyPair::new_account();
    println!(
        "cargo:rustc-env=ISSUER={}",
        issuer.seed().expect("failed to extract issuer seed")
    );

    for (name, caps) in [
        (
            "builtins-compat-reactor",
            Some(vec![
                caps::HTTP_SERVER.into(),
                caps::LOGGING.into(),
                caps::NUMBERGEN.into(),
            ]),
        ),
        (
            "builtins-compat-reactor-preview2",
            Some(vec![
                caps::HTTP_SERVER.into(),
                caps::LOGGING.into(),
                caps::NUMBERGEN.into(),
            ]),
        ),
        (
            "builtins-component-reactor",
            Some(vec![
                caps::HTTP_SERVER.into(),
                caps::LOGGING.into(),
                caps::NUMBERGEN.into(),
            ]),
        ),
        (
            "builtins-component-reactor-preview2",
            Some(vec![
                caps::HTTP_SERVER.into(),
                caps::LOGGING.into(),
                caps::NUMBERGEN.into(),
            ]),
        ),
        (
            "builtins-module-reactor",
            Some(vec![
                caps::HTTP_SERVER.into(),
                caps::LOGGING.into(),
                caps::NUMBERGEN.into(),
            ]),
        ),
        ("http-compat-command", Some(vec![caps::HTTP_SERVER.into()])),
        (
            "http-compat-command-preview2",
            Some(vec![caps::HTTP_SERVER.into()]),
        ),
        ("logging-module-command", Some(vec![caps::LOGGING.into()])),
        ("tcp-component-command", None),
        ("tcp-component-command-preview2", None),
    ] {
        let wasm = fs::read(out_dir.join(format!("rust-{name}.wasm")))
            .await
            .with_context(|| format!("failed to read `{name}` Wasm"))?;
        let module = KeyPair::new_module();
        let claims = ClaimsBuilder::new()
            .issuer(&issuer.public_key())
            .subject(&module.public_key())
            .with_metadata(jwt::Actor {
                name: Some(name.into()),
                caps,
                ..Default::default()
            })
            .build();
        let wasm = embed_claims(&wasm, &claims, &issuer).context("failed to embed actor claims")?;
        fs::write(out_dir.join(format!("rust-{name}.signed.wasm")), wasm)
            .await
            .context("failed to write Wasm")?;
    }

    Ok(())
}
