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

//#[macro_use]
//extern crate log;

use anyhow::anyhow;
use log::info;
use std::result::Result;
use wasmbus_rpc::provider::HostNatsConnection;

mod rpc;
use crate::rpc::HelloProvider;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    let _ = env_logger::try_init();

    let host_data = wasmbus_rpc::provider::load_host_data()?;
    let (tx, rx) = tokio::sync::oneshot::channel();

    info!(
        "Starting Hello Capability Provider {}",
        &host_data.provider_key
    );

    // TODO: get real nats address and credentials from the host/env
    let nc = nats::connect("0.0.0.0:4222").map_err(|e| anyhow!("nats connection failed: {}", e))?;
    let service = Box::new(HelloProvider::default());

    let host = HostNatsConnection::new(nc.clone());
    host.connect(&host_data.provider_key, &host_data.link_name, service, tx)
        .map_err(|e| anyhow!("fatal error setting up subscriptions: {}", e))?;

    println!("Hello provider is ready for requests");
    //p.park();

    let _wait_for_shutdown = rx.await;

    // process pending messages and unsubscribe all subscriptions
    let _ = nc.drain();

    Ok(())
}
