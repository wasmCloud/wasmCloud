use std::env;
use std::process::ExitStatus;

use anyhow::{Context, Result};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use url::Url;

use super::{free_port, spawn_server};

/// Spawn a minio server to use for testing
pub async fn start_minio() -> Result<(
    JoinHandle<Result<ExitStatus>>,
    oneshot::Sender<()>,
    tempfile::TempDir,
    Url,
)> {
    let port = free_port().await?;
    let host = "127.0.0.1";
    let address = format!("{host}:{port}");
    let data_dir = tempfile::tempdir().context("failed to create temp dir for minio")?;
    let (server, stop_tx) = spawn_server(
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
        stop_tx,
        data_dir,
        format!("http://{address}")
            .parse()
            .context("failed to parse URL for minio")?,
    ))
}
