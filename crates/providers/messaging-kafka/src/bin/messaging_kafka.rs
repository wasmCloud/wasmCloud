//! Kafka implementation for wasmcloud:messaging.

use wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk;

use wasmcloud_provider_kafka::KafkaMessagingProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    wasmcloud_provider_sdk::start_provider(
        KafkaMessagingProvider::default(),
        "kafka-messaging-provider",
    )?;

    eprintln!("Kafka messaging provider exiting");
    Ok(())
}
