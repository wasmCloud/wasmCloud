use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Output, Stdio};

use anyhow::{anyhow, bail, ensure, Context};
use nkeys::KeyPair;
use provider_archive::ProviderArchive;
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

trait DerefArtifact {
    fn deref_artifact(&self) -> Option<(&str, &[PathBuf])>;
}

impl DerefArtifact for Option<(String, Vec<PathBuf>)> {
    fn deref_artifact(&self) -> Option<(&str, &[PathBuf])> {
        self.as_ref()
            .map(|(pkg, paths)| (pkg.as_str(), paths.as_slice()))
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../crates/providers");

    let out_dir = env::var("OUT_DIR")
        .map(PathBuf::from)
        .context("failed to lookup `OUT_DIR`")?;

    let issuer = KeyPair::new_account();
    println!(
        "cargo:rustc-env=ISSUER={}",
        issuer.seed().expect("failed to extract issuer seed")
    );
    let mut artifacts = build_artifacts(
        [
            "--manifest-path=../../crates/providers/Cargo.toml",
            "-p=wasmcloud-provider-httpserver",
        ],
        |name, kind| ["httpserver"].contains(&name) && kind.contains(&CrateType::Bin),
    )
    .await
    .context("failed to build `wasmcloud-provider-httpserver` crate")?;
    match (artifacts.next().deref_artifact(), artifacts.next()) {
        (Some(("httpserver", [rust_httpserver])), None) => {
            let mut par = ProviderArchive::new(
                "wasmcloud:httpserver",
                "wasmcloud-provider-httpserver",
                "test",
                None,
                None,
            );
            let bin = fs::read(rust_httpserver)
                .await
                .context("failed to read `wasmcloud-provider-httpserver` binary")?;
            par.add_library(
                &format!(
                    "{}-{}",
                    env::var("CARGO_CFG_TARGET_ARCH").expect("`CARGO_CFG_TARGET_ARCH` not set"),
                    env::var("CARGO_CFG_TARGET_OS").expect("`CARGO_CFG_TARGET_OS` not set")
                ),
                &bin,
            )
            .map_err(|e| {
                anyhow!(e).context("failed to add `wasmcloud-provider-httpserver` binary to PAR")
            })?;
            let subject = KeyPair::new_service();
            println!(
                "cargo:rustc-env=RUST_HTTPSERVER_SUBJECT={}",
                subject.seed().expect("failed to extract subject seed")
            );
            par.write(
                out_dir.join("rust-httpserver.par"),
                &issuer,
                &subject,
                false,
            )
            .await
            .map_err(|e| anyhow!(e).context("failed to write `wasmcloud-provider-httpserver` PAR"))
        }
        _ => bail!("invalid `wasmcloud-provider-httpserver` build artifacts"),
    }
}
