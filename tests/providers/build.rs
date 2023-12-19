use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use anyhow::{anyhow, bail, ensure, Context};
use nkeys::KeyPair;
use provider_archive::ProviderArchive;
use serde::Deserialize;
use tokio::process::Command;
use tokio::{fs, try_join};

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

async fn build_par(
    issuer: &KeyPair,
    out: impl AsRef<Path>,
    capid: impl AsRef<str>,
    name: impl AsRef<str>,
    bin: impl AsRef<Path>,
) -> anyhow::Result<String> {
    let mut par = ProviderArchive::new(capid.as_ref(), name.as_ref(), "test", None, None);
    let bin = bin.as_ref();
    let bin = fs::read(bin)
        .await
        .with_context(|| format!("failed to read binary at `{}`", bin.display()))?;
    par.add_library(
        &format!(
            "{}-{}",
            env::var("CARGO_CFG_TARGET_ARCH").expect("`CARGO_CFG_TARGET_ARCH` not set"),
            env::var("CARGO_CFG_TARGET_OS").expect("`CARGO_CFG_TARGET_OS` not set")
        ),
        &bin,
    )
    .map_err(|e| anyhow!(e).context("failed to add  binary to PAR"))?;
    let subject = KeyPair::new_service();
    let seed = subject.seed().context("failed to extract subject seed")?;
    par.write(out, issuer, &subject, false)
        .await
        .map_err(|e| anyhow!(e).context("failed to write PAR"))?;
    Ok(seed)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=../../crates/provider-sdk");
    println!("cargo:rerun-if-changed=../../crates/providers");
    println!("cargo:rerun-if-changed=build.rs");

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
            "-p=wasmcloud-provider-blobstore-fs",
            "-p=wasmcloud-provider-blobstore-s3",
            "-p=wasmcloud-provider-httpclient",
            "-p=wasmcloud-provider-httpserver",
            "-p=wasmcloud-provider-kv-vault",
            "-p=wasmcloud-provider-kvredis",
            "-p=wasmcloud-provider-nats",
            "-p=wasmcloud-provider-lattice-controller",
        ],
        |name, kind| {
            [
                "blobstore_fs",
                "blobstore_s3",
                "httpclient",
                "httpserver",
                "kv-vault",
                "kvredis",
                "lattice-controller",
                "nats_messaging",
            ]
            .contains(&name)
                && kind.contains(&CrateType::Bin)
        },
    )
    .await
    .context("failed to build provider workspace")?;
    match (
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next().deref_artifact(),
        artifacts.next(),
    ) {
        (
            Some(("blobstore_fs", [rust_blobstore_fs])),
            Some(("blobstore_s3", [rust_blobstore_s3])),
            Some(("httpclient", [rust_httpclient])),
            Some(("httpserver", [rust_httpserver])),
            Some(("kv-vault", [rust_kv_vault])),
            Some(("kvredis", [rust_kvredis])),
            Some(("lattice-controller", [rust_lattice_controller])),
            Some(("nats_messaging", [rust_nats])),
            None,
        ) => {
            let (
                rust_blobstore_fs_seed,
                rust_blobstore_s3_seed,
                rust_httpclient_seed,
                rust_httpserver_seed,
                rust_kvredis_seed,
                rust_kv_vault_seed,
                rust_lattice_controller_seed,
                rust_nats_seed,
            ) = try_join!(
                build_par(
                    &issuer,
                    out_dir.join("rust-blobstore-fs.par"),
                    "wasmcloud:blobstore",
                    "wasmcloud-provider-blobstore-fs",
                    rust_blobstore_fs,
                ),
                build_par(
                    &issuer,
                    out_dir.join("rust-blobstore-s3.par"),
                    "wasmcloud:blobstore",
                    "wasmcloud-provider-blobstore-s3",
                    rust_blobstore_s3,
                ),
                build_par(
                    &issuer,
                    out_dir.join("rust-httpclient.par"),
                    "wasmcloud:httpclient",
                    "wasmcloud-provider-httpclient",
                    rust_httpclient,
                ),
                build_par(
                    &issuer,
                    out_dir.join("rust-httpserver.par"),
                    "wasmcloud:httpserver",
                    "wasmcloud-provider-httpserver",
                    rust_httpserver,
                ),
                build_par(
                    &issuer,
                    out_dir.join("rust-kvredis.par"),
                    "wasmcloud:keyvalue",
                    "wasmcloud-provider-kvredis",
                    rust_kvredis,
                ),
                build_par(
                    &issuer,
                    out_dir.join("rust-kv-vault.par"),
                    "wasmcloud:keyvalue",
                    "wasmcloud-provider-kv-vault",
                    rust_kv_vault,
                ),
                build_par(
                    &issuer,
                    out_dir.join("rust-lattice-controller.par"),
                    "wasmcloud:latticecontrol",
                    "wasmcloud-provider-lattice-controller",
                    rust_lattice_controller,
                ),
                build_par(
                    &issuer,
                    out_dir.join("rust-nats.par"),
                    "wasmcloud:messaging",
                    "wasmcloud-provider-nats",
                    rust_nats,
                ),
            )?;
            println!("cargo:rustc-env=RUST_BLOBSTORE_FS_SUBJECT={rust_blobstore_fs_seed}");
            println!("cargo:rustc-env=RUST_BLOBSTORE_S3_SUBJECT={rust_blobstore_s3_seed}");
            println!("cargo:rustc-env=RUST_HTTPCLIENT_SUBJECT={rust_httpclient_seed}");
            println!("cargo:rustc-env=RUST_HTTPSERVER_SUBJECT={rust_httpserver_seed}");
            println!("cargo:rustc-env=RUST_KVREDIS_SUBJECT={rust_kvredis_seed}");
            println!("cargo:rustc-env=RUST_KV_VAULT_SUBJECT={rust_kv_vault_seed}");
            println!(
                "cargo:rustc-env=RUST_LATTICE_CONTROLLER_SUBJECT={rust_lattice_controller_seed}"
            );
            println!("cargo:rustc-env=RUST_NATS_SUBJECT={rust_nats_seed}");
            Ok(())
        }
        _ => bail!("invalid provider build artifacts"),
    }
}
