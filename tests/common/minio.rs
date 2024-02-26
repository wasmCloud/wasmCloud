use std::env;

use anyhow::{Context, Result};
use tokio::process::Command;
use url::Url;

use super::{free_port, BackgroundServer};

/// Spawn a minio server to use for testing
pub async fn start_minio() -> Result<(BackgroundServer, tempfile::TempDir, Url)> {
    let port = free_port().await?;
    let host = "127.0.0.1";
    let address = format!("{host}:{port}");
    let data_dir = tempfile::tempdir().context("failed to create temp dir for minio")?;
    let server = BackgroundServer::spawn(
        Command::new(env::var("TEST_MINIO_BIN").as_deref().unwrap_or("minio")).args([
            "server",
            "--address",
            address.as_ref(),
            format!("{}", data_dir.path().display()).as_ref(),
        ]),
    )
    .await
    .context("failed to start minio")?;
    Ok((
        server,
        data_dir,
        format!("http://{address}")
            .parse()
            .context("failed to parse URL for minio")?,
    ))
}
