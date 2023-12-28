//! NATS implementation for wasmcloud:messaging.

use wasmcloud_provider_sdk::{load_host_data, start_provider};
use wasmcloud_provider_nats::NatsMessagingProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    start_provider(
        NatsMessagingProvider::from_host_data(load_host_data()?),
        Some("nats-messaging-provider".to_string()),
    )?;

    eprintln!("NATS messaging provider exiting");
    Ok(())
}
