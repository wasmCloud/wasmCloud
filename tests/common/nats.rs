use std::env;
use std::process::ExitStatus;

use anyhow::{Context, Result};
use async_nats::connection::State;
use async_nats::Client as NatsClient;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout, Duration};
use url::Url;

use super::{free_port, spawn_server, tempdir};

pub async fn start_nats() -> Result<(
    JoinHandle<Result<ExitStatus>>,
    oneshot::Sender<()>,
    Url,
    NatsClient,
)> {
    let port = free_port().await?;
    let url =
        Url::parse(&format!("nats://localhost:{port}")).context("failed to parse NATS URL")?;
    let jetstream_dir = tempdir()?;
    let (server, stop_tx) = spawn_server(
        Command::new(
            env::var("WASMCLOUD_NATS")
                .as_deref()
                .unwrap_or("nats-server"),
        )
        .args(["-js", "-D", "-T=false", "-p", &port.to_string(), "-sd"])
        .arg(jetstream_dir.path()),
    )
    .await
    .context("failed to start NATS")?;

    // Wait until nats is ready to take connections
    let nats_client = async_nats::connect_with_options(
        url.as_str(),
        async_nats::ConnectOptions::new().retry_on_initial_connect(),
    )
    .await
    .context("failed to build nats client")?;
    let nats_client = timeout(Duration::from_secs(3), async move {
        loop {
            if nats_client.connection_state() == State::Connected {
                return nats_client;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("failed to ensure connection to NATS server")?;

    Ok((server, stop_tx, url, nats_client))
}
