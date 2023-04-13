use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

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
    } = Command::new("cargo")
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

async fn install_wasi_adapter(out_dir: impl AsRef<Path>) -> anyhow::Result<()> {
    let mut artifacts = build_artifacts(
        ["--manifest-path=../wasi-adapter/Cargo.toml", "-Z=bindeps"],
        |name, kind| name == "wasi_snapshot_preview1" && kind.contains(&CrateType::Cdylib),
    )
    .await
    .context("failed to build `wasi-adapter` crate")?;
    match (artifacts.next().deref_artifact(), artifacts.next()) {
        (Some(("wasi_snapshot_preview1", [path])), None) => {
            copy(path, out_dir.as_ref().join("wasi-snapshot-preview1.wasm"))
                .await
                .map(|_| ())
        }
        _ => bail!("invalid `wasi-snapshot-preview1` build artifacts"),
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
            "-p=actor-echo-module",
            "-p=actor-http-log-rng-module",
        ],
        |name, kind| {
            ["actor-echo-module", "actor-http-log-rng-module"].contains(&name)
                && kind.contains(&CrateType::Cdylib)
        },
    )
    .await
    .context("failed to build `wasm32-unknown-unknown` actors")?;
    match (
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next(),
    ) {
        (
            Some(("actor-echo-module", [echo_path])),
            Some(("actor-http-log-rng-module", [http_log_rng_path])),
            None,
        ) => {
            try_join!(
                copy(echo_path, out_dir.join("actor-rust-echo-module.wasm")),
                copy(
                    http_log_rng_path,
                    out_dir.join("actor-rust-http-log-rng-module.wasm"),
                )
            )?;
            Ok(())
        }
        _ => bail!("invalid `wasm32-unknown-unknown` Rust actor build artifacts"),
    }
}

async fn install_rust_wasm32_wasi_actors(out_dir: impl AsRef<Path>) -> anyhow::Result<()> {
    let out_dir = out_dir.as_ref();
    let mut artifacts = build_artifacts(
        [
            "--manifest-path=./rust/Cargo.toml",
            "--target=wasm32-wasi",
            "-p=actor-foobar-component",
            "-p=actor-foobar-guest-component",
            "-p=actor-foobar-host-component",
            "-p=actor-http-log-rng-component",
        ],
        |name, kind| {
            [
                "actor-foobar-component",
                "actor-foobar-guest-component",
                "actor-foobar-host-component",
                "actor-http-log-rng-component",
            ]
            .contains(&name)
                && kind.contains(&CrateType::Cdylib)
        },
    )
    .await
    .context("failed to build `wasm32-wasi` actors")?;
    match (
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next(),
    ) {
        (
            Some(("actor-foobar-component", [foobar_path])),
            Some(("actor-foobar-guest-component", [foobar_guest_path])),
            Some(("actor-foobar-host-component", [foobar_host_path])),
            Some(("actor-http-log-rng-component", [http_log_rng_path])),
            None,
        ) => {
            try_join!(
                copy(
                    foobar_path,
                    out_dir.join("actor-rust-foobar-component.wasm")
                ),
                copy(
                    foobar_guest_path,
                    out_dir.join("actor-rust-foobar-guest-component.wasm")
                ),
                copy(
                    foobar_host_path,
                    out_dir.join("actor-rust-foobar-host-component.wasm")
                ),
                copy(
                    http_log_rng_path,
                    out_dir.join("actor-rust-http-log-rng-component.wasm"),
                )
            )?;
            Ok(())
        }
        _ => bail!("invalid `wasm32-wasi` Rust actor build artifacts"),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=../wasi-adapter");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=rust");

    let out_dir = env::var("OUT_DIR")
        .map(PathBuf::from)
        .context("failed to lookup `OUT_DIR`")?;
    try_join!(
        install_wasi_adapter(&out_dir),
        install_rust_wasm32_unknown_unknown_actors(&out_dir),
        install_rust_wasm32_wasi_actors(&out_dir),
    )?;
    Ok(())
}
