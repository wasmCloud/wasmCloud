use std::env;
use std::process::ExitStatus;

use anyhow::{Context, Result};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use url::Url;

use super::{free_port, spawn_server};

pub async fn start_redis() -> Result<(JoinHandle<Result<ExitStatus>>, oneshot::Sender<()>, Url)> {
    let port = free_port().await?;
    let url =
        Url::parse(&format!("redis://localhost:{port}")).context("failed to parse Redis URL")?;
    let (server, stop_tx) = spawn_server(
        Command::new(
            env::var("WASMCLOUD_REDIS")
                .as_deref()
                .unwrap_or("redis-server"),
        )
        .args([
            "--port",
            &port.to_string(),
            // Ensure that no data is saved locally, since users with
            // redis-server installed on their machines may have default
            // configurations which normally specify a persistence directory
            "--save",
            "",
            "--dbfilename",
            format!("test-redis-{port}.rdb").as_str(),
        ]),
    )
    .await
    .context("failed to start Redis")?;
    Ok((server, stop_tx, url))
}
