use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::fs;
use tokio::process::Command;

#[tokio::main]
async fn main() -> Result<()> {
    println!("cargo:rerun-if-changed=rust/external-ping/src");
    println!("cargo:rerun-if-changed=rust/external-ping/Cargo.toml");

    let out_dir = env::var("OUT_DIR")
        .map(PathBuf::from)
        .context("failed to lookup `OUT_DIR`")?;

    let external_ping_dir = format!("{}/rust/external-ping", env!("CARGO_MANIFEST_DIR"));
    let status = Command::new("cargo")
        .args(["build", "--release", "--bin", "external-ping"])
        .current_dir(&external_ping_dir)
        .status()
        .await
        .context("failed to build external-ping")?;
    if !status.success() {
        anyhow::bail!("external-ping build failed");
    }

    let src = PathBuf::from(format!(
        "{}/target/release/external-ping",
        external_ping_dir
    ));
    let dst = out_dir.join("external-ping");
    fs::copy(&src, &dst).await.with_context(|| {
        format!(
            "failed to copy external-ping from `{}` to `{}`",
            src.display(),
            dst.display()
        )
    })?;

    Ok(())
}
