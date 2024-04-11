//! NATS implementation for wasmcloud:messaging.

mod connection;
mod nats;

use nats::NatsMessagingProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    NatsMessagingProvider::run().await?;
    eprintln!("NATS messaging provider exiting");
    Ok(())
}
