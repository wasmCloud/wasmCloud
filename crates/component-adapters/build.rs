use std::env::{self, VarError};
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context};
use base64::Engine;
use futures::{try_join, TryStreamExt};
use once_cell::sync::Lazy;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::{tempfile, NamedTempFile};
use tokio::fs::File;
use tokio::{fs, io};
use tokio_util::io::StreamReader;

#[derive(Deserialize)]
enum LockNodeEntryType {
    #[serde(rename = "file")]
    File,
}

#[derive(Deserialize)]
struct LockNodeEntry {
    #[serde(rename = "narHash")]
    nar_hash: String,
    #[serde(rename = "type")]
    typ: LockNodeEntryType,
    url: String,
}

#[derive(Deserialize)]
struct LockNode {
    locked: LockNodeEntry,
}

#[derive(Deserialize)]
struct LockNodes {
    #[serde(rename = "wasi-preview1-command-component-adapter")]
    wasi_preview1_command_component_adapter: LockNode,

    #[serde(rename = "wasi-preview1-reactor-component-adapter")]
    wasi_preview1_reactor_component_adapter: LockNode,
}

#[derive(Deserialize)]
struct Lock {
    nodes: LockNodes,
}

static LOCK: Lazy<Lock> = Lazy::new(|| {
    serde_json::from_str(include_str!("../../flake.lock")).expect("failed to parse `flake.lock`")
});

static WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER_LOCK: Lazy<&LockNodeEntry> =
    Lazy::new(|| &LOCK.nodes.wasi_preview1_command_component_adapter.locked);

static WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER_LOCK: Lazy<&LockNodeEntry> =
    Lazy::new(|| &LOCK.nodes.wasi_preview1_reactor_component_adapter.locked);

struct DigestReader<T> {
    inner: T,
    hash: Sha256,
}

impl<T: Read> Read for DigestReader<T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hash.update(&buf[..n]);
        Ok(n)
    }
}

impl<T> From<T> for DigestReader<T> {
    fn from(inner: T) -> Self {
        Self {
            inner,
            hash: Sha256::default(),
        }
    }
}

fn matches_nar_digest(path: impl AsRef<Path>, expected: impl AsRef<[u8]>) -> anyhow::Result<bool> {
    let mut nar = tempfile().context("failed to create a temporary file")?;
    let mut enc = DigestReader::from(nix_nar::Encoder::new(path));
    std::io::copy(&mut enc, &mut nar).context("failed to encode NAR")?;
    Ok(enc.hash.finalize()[..] == *expected.as_ref())
}

async fn upsert_artifact(
    var: impl AsRef<str>,
    entry: &Lazy<&LockNodeEntry>,
    dst: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let var = var.as_ref();
    match env::var(var) {
        Ok(path) => {
            println!("cargo:rustc-env={var}={path}");
            Ok(())
        }
        Err(VarError::NotUnicode(path)) => {
            bail!("`{var}` value `{path:?}` is not valid unicode")
        }
        Err(VarError::NotPresent) => match entry.typ {
            LockNodeEntryType::File => {
                let dst = dst.as_ref();

                let nar_hash = entry.nar_hash.strip_prefix("sha256-").with_context(|| {
                    format!(
                        "failed to trim `sha256-` prefix from `nar_hash` value of `{}`",
                        entry.nar_hash
                    )
                })?;
                let nar_hash = base64::engine::general_purpose::STANDARD
                    .decode(nar_hash)
                    .context("failed to decode NAR hash from lock")?;

                if dst.exists() {
                    if matches_nar_digest(dst, &nar_hash)? {
                        println!("cargo:rustc-env={var}={}", dst.display());
                        return Ok(());
                    }
                    println!(
                        "cargo:warning=hash mismatch for {}, fetch from upstream",
                        dst.display()
                    );
                }

                let url = &entry.url;
                let res = reqwest::get(url)
                    .await
                    .with_context(|| format!("`{url}` is not a valid URL"))?
                    .error_for_status()
                    .with_context(|| format!("failed to send an HTTP request to `{url}`"))?;

                let wasm = NamedTempFile::new().context("failed to create a temporary file")?;
                let file = wasm.reopen().context("failed to reopen file")?;

                let body = res
                    .bytes_stream()
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
                io::copy(&mut StreamReader::new(body), &mut File::from_std(file))
                    .await
                    .with_context(|| {
                        format!("failed to fetch `{url}` to `{}`", wasm.path().display())
                    })?;
                ensure!(
                    matches_nar_digest(wasm.path(), nar_hash)?,
                    "hash mismatch for `{url}`"
                );

                fs::copy(wasm.path(), dst)
                    .await
                    .with_context(|| format!("failed to copy bytes to `{}`", dst.display()))?;
                println!("cargo:rustc-env={var}={}", dst.display());
                Ok(())
            }
        },
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("cargo:rerun-if-changed=../../flake.lock");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER");
    println!("cargo:rerun-if-env-changed=WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER");

    let out_dir = env::var("OUT_DIR")
        .map(PathBuf::from)
        .context("failed to lookup `OUT_DIR`")?;
    try_join!(
        upsert_artifact(
            "WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER",
            &WASI_PREVIEW1_COMMAND_COMPONENT_ADAPTER_LOCK,
            out_dir.join("wasi_snapshot_preview1.command.wasm")
        ),
        upsert_artifact(
            "WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER",
            &WASI_PREVIEW1_REACTOR_COMPONENT_ADAPTER_LOCK,
            out_dir.join("wasi_snapshot_preview1.reactor.wasm")
        ),
    )?;
    Ok(())
}
