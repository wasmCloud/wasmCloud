use std::env;

use anyhow::{Context, Result};
use async_nats::connection::State;
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::{sleep, timeout, Duration};
use url::Url;

use super::{free_port, tempdir, BackgroundServer};

pub async fn start_nats(
    config: Option<String>,
    return_client: bool,
) -> Result<(BackgroundServer, Url, Option<async_nats::Client>)> {
    let port = free_port().await?;
    let port = port.to_string();
    let url =
        Url::parse(&format!("nats://localhost:{port}")).context("failed to parse NATS URL")?;
    let tmp_dir = tempdir().context("failed to create temporary directory")?;
    let jetstream_dir = tmp_dir.path().join("jetstream").display().to_string();
    let nats_config_path = tmp_dir.path().join("config.json").display().to_string();
    let nats_pid_path = tmp_dir.path().join("nats.pid").display().to_string();

    let mut nats_server_args = vec![
        "-js",
        "-sd",
        &jetstream_dir,
        "-p",
        &port,
        "-P",
        &nats_pid_path,
        "-D",
        "-T=false",
    ];
    if let Some(cfg) = config {
        tokio::fs::write(nats_config_path.clone(), cfg)
            .await
            .context("failed to write config.json")?;
        nats_server_args.push("--config");
        nats_server_args.push(&nats_config_path);
    }

    let server = BackgroundServer::spawn(
        Command::new(
            env::var("TEST_NATS_BIN")
                .as_deref()
                .unwrap_or("nats-server"),
        )
        .args(nats_server_args),
    )
    .await
    .context("failed to start NATS")?;

    // Wait until nats is ready to take connections
    let ensure_connection_timeout = Duration::from_secs(5);
    let client = if return_client {
        let client = async_nats::connect_with_options(
            url.as_str(),
            async_nats::ConnectOptions::new().retry_on_initial_connect(),
        )
        .await
        .context("failed to build NATS client")?;
        Some(ensure_nats_connection_until_timeout(client, ensure_connection_timeout).await?)
    } else {
        // Check to see if the NATS Server is listening on the address to ensure that temporary file doesn't go out of scope
        timeout(ensure_connection_timeout, async move {
            loop {
                let stream = TcpStream::connect(format!("localhost:{port}")).await;
                if let Ok(stream) = stream {
                    if stream.readable().await.is_ok() {
                        return;
                    }
                }
                sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .context("failed to ensure connection to NATS was available")?;
        None
    };

    Ok((server, url, client))
}

pub async fn ensure_nats_connection_until_timeout(
    client: async_nats::Client,
    timeout: Duration,
) -> Result<async_nats::Client> {
    tokio::time::timeout(timeout, async move {
        loop {
            if client.connection_state() == State::Connected {
                return Ok(client);
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("failed to ensure connection to NATS server")?
}
