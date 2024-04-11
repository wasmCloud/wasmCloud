//! Kafka implementation for wasmcloud:messaging.

use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_messaging_kafka::run()
        .await
        .context("failed to run provider")?;
    eprintln!("Kafka messaging provider exiting");
    Ok(())
}
