use anyhow::{Context as _, Result};
use async_nats::{Client, ToServerAddrs};
use tokio::time::Duration;

/// Wait for a given NATS connection to be available
pub async fn wait_for_nats_connection(url: impl ToServerAddrs) -> Result<Client> {
    tokio::time::timeout(Duration::from_secs(3), async move {
        loop {
            if let Ok(c) = async_nats::connect(&url).await {
                return c;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .context("failed to connect NATS server client")
}
