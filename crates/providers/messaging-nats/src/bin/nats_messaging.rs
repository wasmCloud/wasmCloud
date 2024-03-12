//! NATS implementation for wasmcloud:messaging.

use wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk;

use wasmcloud_provider_nats::NatsMessagingProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    wasmcloud_provider_sdk::start_provider(
        NatsMessagingProvider::from_host_data(wasmcloud_provider_sdk::load_host_data()?),
        "nats-messaging-provider",
    )?;

    eprintln!("NATS messaging provider exiting");
    Ok(())
}
