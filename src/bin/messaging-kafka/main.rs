//! Kafka implementation for wasmcloud:messaging.

use wasmcloud_provider_messaging_kafka::KafkaMessagingProvider;
use wasmcloud_provider_sdk::start_provider;

fn main() -> anyhow::Result<()> {
    start_provider(
        KafkaMessagingProvider::default(),
        "kafka-messaging-provider",
    )?;
    eprintln!("Kafka messaging provider exiting");
    Ok(())
}
