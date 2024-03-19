//! NATS implementation for wasmcloud:messaging.

use anyhow::Context as _;
use wasmcloud_provider_messaging_nats::NatsMessagingProvider;
use wasmcloud_provider_sdk::{load_host_data, start_provider};

fn main() -> anyhow::Result<()> {
    let host_data = load_host_data().context("failed to load host data")?;
    start_provider(
        NatsMessagingProvider::from_host_data(host_data),
        "nats-messaging-provider",
    )?;
    eprintln!("NATS messaging provider exiting");
    Ok(())
}
