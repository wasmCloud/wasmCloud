//! NATS implementation for wasmcloud:messaging.

use anyhow::Context as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    wasmcloud_provider_messaging_nats::run()
        .await
        .context("failed to run provider")?;
    eprintln!("NATS messaging provider exiting");
    Ok(())
}
