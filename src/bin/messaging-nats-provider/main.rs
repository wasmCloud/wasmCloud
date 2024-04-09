//! NATS implementation for wasmcloud:messaging.

use wasmcloud_provider_messaging_nats::NatsMessagingProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    NatsMessagingProvider::run().await?;
    eprintln!("NATS messaging provider exiting");
    Ok(())
}
