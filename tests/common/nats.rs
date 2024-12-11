use std::env;

use anyhow::{Context, Result};
use async_nats::connection::State;
use tokio::process::Command;
use tokio::time::{sleep, timeout, Duration};
use url::Url;

use super::{free_port, tempdir, BackgroundServer};

pub async fn start_nats() -> Result<(BackgroundServer, Url, async_nats::Client)> {
    let port = free_port().await?;
    let url =
        Url::parse(&format!("nats://localhost:{port}")).context("failed to parse NATS URL")?;
    let jetstream_dir = tempdir()?;
    let server = BackgroundServer::spawn(
        Command::new(
            env::var("TEST_NATS_BIN")
                .as_deref()
                .unwrap_or("nats-server"),
        )
        .args([
            "-js",
            "-D",
            "-T=false",
            "-p",
            &port.to_string(),
            "-sd",
            jetstream_dir.path().display().to_string().as_str(),
        ]),
    )
    .await
    .context("failed to start NATS")?;

    // Wait until nats is ready to take connections
    let client = async_nats::connect_with_options(
        url.as_str(),
        async_nats::ConnectOptions::new().retry_on_initial_connect(),
    )
    .await
    .context("failed to build NATS client")?;
    let client = timeout(Duration::from_secs(3), async move {
        loop {
            if client.connection_state() == State::Connected {
                return client;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("failed to ensure connection to NATS server")?;

    let client = timeout(Duration::from_secs(3), async move {
        loop {
            if client.connection_state() == State::Connected {
                return client;
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("failed to ensure connection to NATS server")?;

    Ok((server, url, client))
}
