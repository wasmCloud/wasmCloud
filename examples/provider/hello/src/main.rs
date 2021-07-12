//! # wasmCloud Hello Provider
//!

use anyhow::anyhow;
use log::info;
use once_cell::sync::OnceCell;
use std::time::Duration;
use tokio::sync::oneshot::error::TryRecvError;
use wasmbus_rpc::provider::{HostBridge, ProviderHandler};
use wasmbus_rpc::{
    core::{HealthCheckRequest, HealthCheckResponse},
    RpcError,
};

mod hello_rpc;
use crate::hello_rpc::HelloProvider;

static BRIDGE: OnceCell<HostBridge> = OnceCell::new();

/// Your provider can handle any of these methods
/// to receive notification of new actor links, deleted links,
/// and for handling health check.
/// The default handlers are implemented in the trait ProviderHandler.
impl ProviderHandler for HelloProvider {
    /// Perform health check. Called at regular intervals by host
    fn health_request(&self, _arg: &HealthCheckRequest) -> Result<HealthCheckResponse, RpcError> {
        Ok(HealthCheckResponse {
            healthy: true,
            message: None,
        })
    }

    /*
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    /// This message is idempotent - provider must be able to handle
    /// duplicates
    fn put_link(&self, ld: &LinkDefinition) -> Result<bool, RpcError> {
        Ok(true)
    }
     */

    /*
    /// Notify the provider that the link is dropped
    fn delete_link(&self, actor_id: &str) {}
     */

    /*
    /// Handle system shutdown message
    fn shutdown(&self) -> Result<(), Infallible> {
        Ok(())
    }
     */
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    pretty_env_logger::init();
    // create logging channel so all logs can be sent from main thread.
    // The channel bound is 0 so that a thread will block until it's been logged.
    let (log_tx, log_rx) = crossbeam::channel::bounded(0);

    info!("Initializing hello provider");

    let host_data = wasmbus_rpc::provider::load_host_data()?;
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
    info!(
        "Starting Hello Capability Provider {}\n",
        &host_data.provider_key
    );
    // TODO: get real nats address and credentials from the host/env
    let nc = nats::connect("0.0.0.0:4222").map_err(|e| anyhow!("nats connection failed: {}", e))?;

    let provider = HelloProvider::default();

    let _ = BRIDGE.set(HostBridge::new(nc.clone(), log_tx));

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

    info!("Hello provider is ready for requests (8)");

    // every 50ms, check for new log messages.
    let timeout = Duration::from_millis(50);
    loop {
        match log_rx.recv_timeout(timeout) {
            // if we have received a log message, continue
            // so that we check immediately - a group of messages that
            // arrive together will be printed with no delay between them.
            Ok((level, s)) => {
                log::logger().log(
                    &log::Record::builder()
                        .args(format_args!("{}", s))
                        .level(level)
                        .build(),
                );
                continue;
            }
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                eprintln!("** Logger exited - quitting **");
                break;
            }
        }

        // on each iteration of the loop, after all pending logs
        // have been written out, check for shutdown signal
        match shutdown_rx.try_recv() {
            Ok(_) => {
                // got the shutdown signal
                break;
            }
            Err(TryRecvError::Empty) => {
                // no signal yet, keep waiting
            }
            Err(TryRecvError::Closed) => {
                // sender exited
                break;
            }
        }
    }
    //let _wait_for_shutdown = shutdown_rx.await;
    eprintln!("********** Hello server exiting");

    // Flush outgoing buffers and unsubscribe all subscriptions.
    // Most of the subscriptions have alrady been drained.
    let _ = nc.drain();

    Ok(())
}
