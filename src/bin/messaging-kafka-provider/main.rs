//! Kafka implementation for wasmcloud:messaging.

use wasmcloud_provider_messaging_kafka::KafkaMessagingProvider;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    KafkaMessagingProvider::run().await?;
    eprintln!("Kafka messaging provider exiting");
    Ok(())
}
