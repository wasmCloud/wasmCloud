//! Http Server implementation for wasmcloud:httpserver
//!
//!
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tracing::{error, instrument, trace, warn};
use wasmcloud_provider_sdk::core::{LinkDefinition, WasmCloudEntity};
use wasmcloud_provider_sdk::error::{InvocationError, ProviderInvocationError};
use wasmcloud_provider_sdk::provider_main::start_provider;
use wasmcloud_provider_sdk::ProviderHandler;

use wasmcloud_provider_httpserver::{load_settings, HttpServerCore, Request, Response};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // handle lattice control messages and forward rpc to the provider dispatch
    // returns when provider receives a shutdown control message
    start_provider(
        HttpServerProvider::default(),
        Some("HttpServer Provider".to_string()),
    )?;

    eprintln!("HttpServer provider exiting");
    Ok(())
}

/// HttpServer provider implementation.
#[derive(Clone, Default)]
struct HttpServerProvider {
    // map to store http server (and its link parameters) for each linked actor
    actors: Arc<dashmap::DashMap<String, HttpServerCore>>,
}

/// Your provider can handle any of these methods
/// to receive notification of new actor links, deleted links,
/// and for handling health check.
/// Default handlers are implemented in the trait ProviderHandler.
#[async_trait]
impl ProviderHandler for HttpServerProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    async fn put_link(&self, ld: &LinkDefinition) -> bool {
        let settings = match load_settings(&ld.values) {
            Ok(s) => s,
            Err(e) => {
                error!(%e, ?ld, "httpserver failed to load settings for actor");
                return false;
            }
        };

        let http_server = HttpServerCore::new(settings.clone(), call_actor);

        if let Err(e) = http_server.start(ld).await {
            error!(%e, ?ld, "httpserver failed to start listener for actor");
            return false;
        }

        self.actors.insert(ld.actor_id.to_string(), http_server);

        true
    }

    /// Handle notification that a link is dropped - stop the http listener
    async fn delete_link(&self, actor_id: &str) {
        if let Some(entry) = self.actors.remove(actor_id) {
            tracing::info!(%actor_id, "httpserver stopping listener for actor");
            entry.1.begin_shutdown();
        }
    }

    /// Handle shutdown request by shutting down all the http server threads
    async fn shutdown(&self) {
        // empty the actor link data and stop all servers
        self.actors.clear();
    }
}

#[async_trait]
impl wasmcloud_provider_sdk::MessageDispatch for HttpServerProvider {
    async fn dispatch<'a>(
        &'a self,
        _: ::wasmcloud_provider_sdk::Context,
        _: String,
        _: std::borrow::Cow<'a, [u8]>,
    ) -> Result<Vec<u8>, ::wasmcloud_provider_sdk::error::ProviderInvocationError> {
        Ok(Vec::with_capacity(0))
    }
}

impl wasmcloud_provider_sdk::Provider for HttpServerProvider {}

const HANDLE_REQUEST_METHOD: &str = "HttpServer.HandleRequest";

pub struct Server<'a> {
    ld: &'a LinkDefinition,
    timeout: Option<std::time::Duration>,
}

impl<'a> Server<'a> {
    pub fn new(ld: &'a LinkDefinition, timeout: Option<Duration>) -> Self {
        Self { ld, timeout }
    }

    pub async fn handle_request(&self, req: Request) -> Result<Response, ProviderInvocationError> {
        let connection = wasmcloud_provider_sdk::provider_main::get_connection();

        let client = connection.get_rpc_client();
        let origin = WasmCloudEntity {
            public_key: self.ld.provider_id.clone(),
            link_name: self.ld.link_name.clone(),
            contract_id: "wasmcloud:httpserver".to_string(),
        };
        let target = WasmCloudEntity {
            public_key: self.ld.actor_id.clone(),
            ..Default::default()
        };

        let data = wasmcloud_provider_sdk::serialize(&req)?;

        let response = if let Some(timeout) = self.timeout {
            client
                .send_timeout(origin, target, HANDLE_REQUEST_METHOD, data, timeout)
                .await?
        } else {
            client
                .send(origin, target, HANDLE_REQUEST_METHOD, data)
                .await?
        };

        if let Some(e) = response.error {
            return Err(ProviderInvocationError::Provider(e));
        }

        let response: Response = wasmcloud_provider_sdk::deserialize(&response.msg)?;

        Ok(response)
    }
}

/// forward Request to actor.
#[instrument(level = "debug", skip_all, fields(actor_id = %ld.actor_id))]
async fn call_actor(
    ld: Arc<LinkDefinition>,
    req: Request,
    timeout: Option<std::time::Duration>,
) -> Result<Response, ProviderInvocationError> {
    let sender = Server::new(&ld, timeout);

    let rc = sender.handle_request(req).await;
    match rc {
        Err(ProviderInvocationError::Invocation(InvocationError::Timeout)) => {
            error!("actor request timed out: returning 503",);
            Ok(Response {
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
