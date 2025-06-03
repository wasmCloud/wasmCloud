use anyhow::{Context, Result};
use tokio::process::Command;
use tokio::time::{sleep, timeout, Duration};
use url::Url;
use vaultrs::client::{Client, VaultClient, VaultClientSettingsBuilder};
use vaultrs::sys::ServerStatus;

use super::{free_port, BackgroundServer};

/// Start Hashicorp Vault as a subprocess on a random port
pub async fn start_vault(token: impl AsRef<str>) -> Result<(BackgroundServer, Url, VaultClient)> {
    let bin_path = std::env::var("TEST_VAULT_BIN").unwrap_or("vault".to_string());
    let port = free_port()
        .await
        .context("failed to find open port for Vault")?;
    let host = "127.0.0.1";
    let server = BackgroundServer::spawn(Command::new(bin_path).args([
        "server",
        "-dev",
        "-dev-listen-address",
        &format!("{host}:{port}"),
        "-dev-root-token-id",
        token.as_ref(),
        "-dev-no-store-token",
    ]))
    .await
    .context("failed to start test Vault instance")?;
    let url = format!("http://{host}:{port}");

    // Create a vault client for use, while waiting for server to start taking connections
    let vault_client = VaultClient::new(
        VaultClientSettingsBuilder::default()
            .address(&url)
            .token(token.as_ref())
            .build()
            .context("failed to build vault client settings")?,
    )
    .context("failed to build vault client")?;
    // NOTE(thomastaylor312): Vault sometimes takes a while to start up, even on local machines. I
    // got to this number by figuring out the time needed on my machine (about 6-7 seconds) and then
    // adding a little extra time to account for GH runner slowness
    let vault_client = timeout(Duration::from_secs(10), async move {
        loop {
            if let Ok(ServerStatus::OK) = vault_client.status().await {
                return vault_client;
            }
            sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .context("failed to ensure connection to vault server")?;

    Ok((
        server,
        url.parse().context("failed to create URL from vault URL")?,
        vault_client,
    ))
}
