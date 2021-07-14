//! # wasmCloud Hello Provider
//!

use anyhow::anyhow;
use log::info;
use once_cell::sync::OnceCell;
use wasmbus_rpc::provider::{wait_for_shutdown, HostBridge};

mod hello_rpc;

/// Hello provider implementation
#[derive(Clone, Debug, Default)]
pub struct HelloProvider {}

static BRIDGE: OnceCell<HostBridge> = OnceCell::new();

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    pretty_env_logger::init();
    // create logging channel so all logs can be sent from main thread.
    // The channel bound is 0 so that a thread will block until it's been logged.
    let (log_tx, log_rx) = crossbeam::channel::bounded(0);

    let host_data = wasmbus_rpc::provider::load_host_data()?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    info!(
        "Starting Hello Capability Provider {}\n",
        &host_data.provider_key
    );
    // TODO: get real nats address and credentials from the host/env
    let nc = nats::connect("0.0.0.0:4222").map_err(|e| anyhow!("nats connection failed: {}", e))?;

    let provider = HelloProvider::default();

    // create bridge to subscribe to host events, issuing callbacks to provider for processing
    let _ = BRIDGE.set(HostBridge::new(nc.clone(), &host_data, log_tx));
    BRIDGE
        .get()
        .unwrap()
        .connect(
            host_data.provider_key.to_string(),
            &host_data.link_name,
            provider,
            shutdown_tx,
        )
        .map_err(|e| anyhow!("fatal error setting up subscriptions: {}", e))?;

    info!("Hello provider is ready for requests");

    // process subscription events and log messages, waiting for shutdown signal
    wait_for_shutdown(log_rx, shutdown_rx);

    // Flush outgoing buffers and unsubscribe from all remaining subscriptions
    let _ = nc.drain();
    info!("Hello provider exiting");

    Ok(())
}
