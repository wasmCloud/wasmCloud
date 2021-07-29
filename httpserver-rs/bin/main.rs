//! Http Server implementation for wasmcloud:httpserver
//!
//!
use wasmcloud_provider_httpserver::{load_settings, Error, HttpServer};

use log::info;
use once_cell::sync::OnceCell;
use std::{collections::HashMap, convert::Infallible, sync::Arc};

use tokio::sync::RwLock;
use wasmbus_rpc::{
    channel_log,
    core::{HealthCheckRequest, HealthCheckResponse, LinkDefinition},
    provider::prelude::*,
    provider::HostBridge,
};

/// singleton host bridge for communicating with the host.
static BRIDGE: OnceCell<HostBridge> = OnceCell::new();

/// nats address to use if not included in initial HostData
const DEFAULT_NATS_ADDR: &str = "0.0.0.0:4222";

// this may be called any time after initialization
pub fn get_bridge() -> &'static HostBridge {
    match BRIDGE.get() {
        Some(b) => b,
        None => {
            // initialized first thing, so this shouldn't happen
            eprintln!("BRIDGE not initialized");
            panic!();
        }
    }
}

/// HttpServer provider implementation.
#[derive(Clone)]
struct HttpServerProvider {
    // map to store http server (and its link parameters) for each linked actor
    actors: Arc<RwLock<HashMap<String, HttpServer>>>,
}

impl HttpServerProvider {
    /// Starts the http server in its own (green) thread.
    /// Triggered by put link definition
    async fn start_server(&self, ld: &LinkDefinition) -> Result<(), Error> {
        let settings = load_settings(&ld.values)?;

        let http_server = HttpServer::new(settings.clone(), get_bridge());
        http_server.start(ld.clone()).await?;

        let mut update_map = self.actors.write().await;
        update_map.insert(ld.actor_id.to_string(), http_server);

        Ok(())
    }

    /// Stops the http server for this actor. Triggered by link delete from host.
    async fn stop(&self, actor_id: &str) {
        let mut aw = self.actors.write().await;
        if let Some(server) = aw.remove(actor_id) {
            info!("httpserver stopping listener for actor {}", actor_id);
            server.begin_shutdown().await;
        }
    }
}

fn main() -> Result<(), Error> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .build()
        .map_err(|e| Error::Init(e.to_string()))?;

    if let Err(e) = runtime.block_on(async {
        if let Err(e) = run().await {
            eprintln!("Error: {}", e.to_string());
            Err(e)
        } else {
            Ok(())
        }
    }) {
        eprintln!("runtime may have exited early: {}", e);
    }
    // in the unlikely case there are any stuck threads,
    // close them so the process has a clean exit
    runtime.shutdown_timeout(core::time::Duration::from_secs(10));
    Ok(())
}

async fn run() -> Result<(), Error> {
    // initialize logger
    let log_rx = channel_log::init_logger()
        .map_err(|_| Error::Init("log already initialized".to_string()))?;
    channel_log::init_receiver(log_rx);

    // get lattice configuration from host
    let host_data =
        wasmbus_rpc::provider::load_host_data().map_err(|e| Error::Init(e.to_string()))?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    info!(
        "Starting HttpServer Capability Provider {} with nats url {}",
        &host_data.provider_key, &host_data.lattice_rpc_url,
    );

    let nats_addr = if !host_data.lattice_rpc_url.is_empty() {
        &host_data.lattice_rpc_url
    } else {
        DEFAULT_NATS_ADDR
    };

    // Connect to nats
    let nc = NatsClient::new(host_data.nats_options())
        .await
        .map_err(|e| Error::Init(format!("nats connection to {} failed: {}", nats_addr, e)))?;

    let provider = HttpServerProvider {
        actors: Arc::new(RwLock::new(HashMap::default())),
    };

    // initialize HostBridge and subscribe to nats topics.
    let bridge = HostBridge::new(nc, &host_data).map_err(|e| Error::Init(e.to_string()))?;
    let _ = BRIDGE.set(bridge);
    let _join = get_bridge()
        .connect(provider, shutdown_tx)
        .await
        .map_err(|e| {
            Error::Init(format!(
                "fatal error setting up subscriptions: {}",
                e.to_string()
            ))
        })?;

    // process subscription events and log messages, waiting for shutdown signal
    let _ = shutdown_rx.await;
    // stop the logger thread
    //let _ = stop_log_thread.send(());
    channel_log::stop_receiver();

    eprintln!("HttpServer provider exiting");

    Ok(())
}

/// Your provider can handle any of these methods
/// to receive notification of new actor links, deleted links,
/// and for handling health check.
/// Default handlers are implemented in the trait ProviderHandler.
#[async_trait]
impl ProviderHandler for HttpServerProvider {
    /// Perform health check. Called at regular intervals by host
    async fn health_request(
        &self,
        _arg: &HealthCheckRequest,
    ) -> Result<HealthCheckResponse, RpcError> {
        Ok(HealthCheckResponse {
            healthy: true,
            message: None,
        })
    }

    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    async fn put_link(&self, ld: &LinkDefinition) -> Result<bool, RpcError> {
        self.start_server(ld)
            .await
            .map_err(|e| RpcError::ProviderInit(e.to_string()))?;
        Ok(true)
    }

    /// Handle notification that a link is dropped - stop the http listener
    async fn delete_link(&self, actor_id: &str) {
        self.stop(actor_id).await;
    }

    /// Handle shutdown request by shutting down all the http server threads
    async fn shutdown(&self) -> Result<(), Infallible> {
        let mut aw = self.actors.write().await;
        // empty the actor link data and stop all servers
        for (_, server) in aw.drain() {
            server.begin_shutdown().await;
        }
        Ok(())
    }
}

/// Handle RPC messages. This provider doesn't have any.
impl ProviderDispatch for HttpServerProvider {}
#[async_trait]
impl MessageDispatch for HttpServerProvider {
    async fn dispatch(
        &self,
        _ctx: &context::Context<'_>,
        message: Message<'_>,
    ) -> Result<Message<'_>, RpcError> {
        // We don't implement an rpc receiver
        Err(RpcError::MethodNotHandled(message.method.to_string()))
    }
}
