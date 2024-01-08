//! Kafka implementation for wasmcloud:messaging.

use wasmcloud_provider_kafka::KafkaMessagingProvider;
use wasmcloud_provider_sdk::start_provider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    start_provider(
        KafkaMessagingProvider::default(),
        Some("kafka-messaging-provider".to_string()),
    )?;

    eprintln!("Kafka messaging provider exiting");
    Ok(())
}
