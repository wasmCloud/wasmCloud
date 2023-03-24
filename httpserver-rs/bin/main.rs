//! Http Server implementation for wasmcloud:httpserver
//!
//!
use std::{convert::Infallible, sync::Arc};

use async_trait::async_trait;
use tracing::{error, instrument, trace, warn};
use wasmbus_rpc::{
    core::LinkDefinition, error::RpcError, provider::prelude::*, provider::ProviderTransport,
};
use wasmcloud_httpserver_provider::{
    load_settings,
    wasmcloud_interface_httpserver::{HttpRequest, HttpResponse, HttpServer, HttpServerSender},
    HttpServerCore,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // handle lattice control messages and forward rpc to the provider dispatch
    // returns when provider receives a shutdown control message
    provider_main(
        HttpServerProvider::default(),
        Some("HttpServer Provider".to_string()),
    )?;

    eprintln!("HttpServer provider exiting");
    Ok(())
}

/// HttpServer provider implementation.
#[derive(Clone, Default, Provider)]
struct HttpServerProvider {
    // map to store http server (and its link parameters) for each linked actor
    actors: Arc<dashmap::DashMap<String, HttpServerCore>>,
}

impl ProviderDispatch for HttpServerProvider {}

/// Your provider can handle any of these methods
/// to receive notification of new actor links, deleted links,
/// and for handling health check.
/// Default handlers are implemented in the trait ProviderHandler.
#[async_trait]
impl ProviderHandler for HttpServerProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    async fn put_link(&self, ld: &LinkDefinition) -> Result<bool, RpcError> {
        let settings =
            load_settings(&ld.values).map_err(|e| RpcError::ProviderInit(e.to_string()))?;

        let http_server = HttpServerCore::new(
            settings.clone(),
            get_host_bridge().lattice_prefix().to_string(),
            call_actor,
        );

        http_server.start(ld).await.map_err(|e| {
            RpcError::ProviderInit(format!(
                "starting httpserver for {} {:?}: {}",
                &ld.actor_id, &settings.address, e
            ))
        })?;

        self.actors.insert(ld.actor_id.to_string(), http_server);

        Ok(true)
    }

    /// Handle notification that a link is dropped - stop the http listener
    async fn delete_link(&self, actor_id: &str) {
        if let Some(entry) = self.actors.remove(actor_id) {
            tracing::info!(%actor_id, "httpserver stopping listener for actor");
            entry.1.begin_shutdown();
        }
    }

    /// Handle shutdown request by shutting down all the http server threads
    async fn shutdown(&self) -> Result<(), Infallible> {
        // empty the actor link data and stop all servers
        self.actors.clear();
        Ok(())
    }
}

/// forward HttpRequest to actor.
#[instrument(level = "debug", skip(_lattice_id, ld, req, timeout), fields(actor_id = %ld.actor_id))]
async fn call_actor(
    _lattice_id: String,
    ld: Arc<LinkDefinition>,
    req: HttpRequest,
    timeout: Option<std::time::Duration>,
) -> Result<HttpResponse, RpcError> {
    let tx = ProviderTransport::new_with_timeout(ld.as_ref(), Some(get_host_bridge()), timeout);
    let ctx = Context::default();
    let actor = HttpServerSender::via(tx);

    let rc = actor.handle_request(&ctx, &req).await;
    match rc {
        Err(RpcError::Timeout(_)) => {
            error!("actor request timed out: returning 503",);
            Ok(HttpResponse {
                status_code: 503,
                body: Default::default(),
                header: Default::default(),
            })
        }

        Ok(resp) => {
            trace!(
                status_code = %resp.status_code,
                "http response received from actor"
            );
            Ok(resp)
        }
        Err(e) => {
            warn!(
                error = %e,
                "actor responded with error"
            );
            Err(e)
        }
    }
}
