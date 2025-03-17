use anyhow::{Context, Result};
use std::env;
use tokio::process::Command;
use url::Url;

use super::{free_port, BackgroundServer};

pub async fn start_wrpc(nats_url: &str) -> Result<BackgroundServer> {
    let binary_path = test_virtual_components::EXTERNAL_PING;

    let server = BackgroundServer::spawn(
        Command::new(binary_path)
            .arg(nats_url)
            .env("RUST_LOG", "debug"),
    )
    .await
    .with_context(|| format!("failed to start external wRPC service at {}", binary_path))?;

    Ok(server)
}
