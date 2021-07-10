//! # wasmCloud Hello Provider
//!
//! Topics relevant to a capability provider:
//!
//! RPC:
//!   * wasmbus.rpc.{prefix}.{provider_key}.{link_name} - Get Invocation, answer InvocationResponse
//!   * wasmbus.rpc.{prefix}.{public_key}.{link_name}.linkdefs.get - Query all link defs for this provider. (queue subscribed)
//!   * wasmbus.rpc.{prefix}.{public_key}.{link_name}.linkdefs.del - Remove a link def. Provider de-provisions resources for the given actor.
//!   * wasmbus.rpc.{prefix}.{public_key}.{link_name}.linkdefs.put - Puts a link def. Provider provisions resources for the given actor.
//!   * wasmbus.rpc.{prefix}.{public_key}.{link_name}.shutdown - Request for graceful shutdown

use anyhow::anyhow;
use log::{info, warn};
use std::result::Result;
use std::sync::Arc;
use wasmbus_rpc::provider::{HostBridge, ProviderHandler};

//use std::sync::mpsc::channel;
mod hello_rpc;
use crate::hello_rpc::HelloProvider;
use once_cell::sync::OnceCell;
use std::time::Duration;
use tokio::sync::oneshot::error::TryRecvError;

static BRIDGE: OnceCell<Arc<HostBridge>> = OnceCell::new();

/// handle link add/delete and shutdown in this trait,
impl ProviderHandler for HelloProvider {}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    //let (log_tx, log_rx) = std::sync::mpsc::channel(); //tokio::sync::mpsc::channel(1);
    let (log_tx, log_rx) = crossbeam::channel::bounded(0);
    pretty_env_logger::init();

    info!("Initializing hello provider");

    let host_data = wasmbus_rpc::provider::load_host_data()?;
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
    info!(
        "Starting Hello Capability Provider {}\n",
        &host_data.provider_key
    );
    // TODO: get real nats address and credentials from the host/env
    let nc = nats::connect("0.0.0.0:4222").map_err(|e| anyhow!("nats connection failed: {}", e))?;

    let provider = Arc::new(HelloProvider::default());

    let _ = BRIDGE.set(Arc::new(HostBridge {
        nats: Some(nc.clone()),
        ..Default::default()
    }));

    BRIDGE
        .get()
        .unwrap()
        .connect(
            host_data.provider_key.to_string(),
            &host_data.link_name,
            provider,
            shutdown_tx,
            log_tx,
        )
        .map_err(|e| anyhow!("fatal error setting up subscriptions: {}", e))?;

    warn!("Hello provider is ready for requests (5)");
    let timeout = Duration::from_millis(50);

    loop {
        match log_rx.recv_timeout(timeout) {
            Ok((level, s)) => {
                //info!("log: {}, {}\r\n", level, s);
                //eprintln!("log: {}, {}", level, s);
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

    // process pending messages and unsubscribe all subscriptions
    let _ = nc.drain();

    Ok(())
}
