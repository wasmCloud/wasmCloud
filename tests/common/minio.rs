use std::env;
use std::ffi::OsStr;
use std::net::Ipv4Addr;

use anyhow::{Context, Result};
use tokio::process::Command;
use url::Url;

use super::{free_port, BackgroundServer};

/// Start Minio as a subprocess on a random port
pub async fn start_minio(path: impl AsRef<OsStr>) -> Result<(BackgroundServer, Url)> {
    let port = free_port().await?;
    let url = Url::parse(&format!("http://{}:{port}", Ipv4Addr::LOCALHOST))
        .context("failed to parse MinIO URL")?;
    Ok((
        BackgroundServer::spawn(
            Command::new(env::var("TEST_MINIO_BIN").as_deref().unwrap_or("minio"))
                .args(["server", "--address"])
                .arg(format!("{}:{port}", Ipv4Addr::LOCALHOST))
                .arg("--console-address")
                .arg(format!("{}:0", Ipv4Addr::LOCALHOST))
                .arg(path),
        )
        .await
        .context("failed to start MinIO")?,
        url,
    ))
}
