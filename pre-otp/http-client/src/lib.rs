///! http-client-provider
///! This library exposes the HTTP client capability to wasmCloud-compliant actors
mod http_client;

use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use codec::core::{OP_BIND_ACTOR, OP_REMOVE_ACTOR, SYSTEM_ACTOR};
use codec::{capability_provider, deserialize};
use http::{RequestArgs, OP_REQUEST};
use log::{info, warn};
use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use wasmcloud_actor_core::CapabilityConfiguration;
use wasmcloud_actor_http_client as http;
use wasmcloud_provider_core as codec;

#[allow(unused)]
const CAPABILITY_ID: &str = "wasmcloud:httpclient";

#[cfg(not(feature = "static_plugin"))]
capability_provider!(HttpClientProvider, HttpClientProvider::new);

/// An implementation HTTP client provider using reqwest.
#[derive(Clone)]
pub struct HttpClientProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    clients: Arc<RwLock<HashMap<String, reqwest::Client>>>,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl HttpClientProvider {
    /// Create a new HTTP client provider.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the HTTP client for a particular actor.
    /// Each actor gets a dedicated client so that we can take advantage of connection pooling.
    /// TODO: This needs to set things like timeouts, redirects, etc.
    fn configure(
        &self,
        config: CapabilityConfiguration,
    ) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
        let timeout = match config.values.get("timeout") {
            Some(v) => {
                let parsed: u64 = v.parse()?;
                Duration::new(parsed, 0)
            }
            None => Duration::new(30, 0),
        };

        let redirect_policy = match config.values.get("max_redirects") {
            Some(v) => {
                let parsed: usize = v.parse()?;
                reqwest::redirect::Policy::limited(parsed)
            }
            None => reqwest::redirect::Policy::default(),
        };

        self.clients.write().unwrap().insert(
            config.module,
            reqwest::Client::builder()
                .timeout(timeout)
                .redirect(redirect_policy)
                .build()?,
        );
        Ok(vec![])
    }

    /// Clean up resources when a actor disconnects.
    /// This removes the HTTP client associated with an actor.
    fn deconfigure(
        &self,
        config: CapabilityConfiguration,
    ) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
        if self
            .clients
            .write()
            .unwrap()
            .remove(&config.module)
            .is_none()
        {
            warn!(
                "attempted to remove non-existent actor: {}",
                config.module.as_str()
            );
        }

        Ok(vec![])
    }

    /// Make a HTTP request.
    fn request(
        &self,
        actor: &str,
        msg: RequestArgs,
    ) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
        let lock = self.clients.read().unwrap();
        let client = lock.get(actor).unwrap();
        self.runtime
            .block_on(async { http_client::request(&client, msg).await })
    }
}

impl Default for HttpClientProvider {
    fn default() -> Self {
        let _ = env_logger::builder().format_module_path(false).try_init();

        let r = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        HttpClientProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            clients: Arc::new(RwLock::new(HashMap::new())),
            runtime: Arc::new(r),
        }
    }
}

/// Implements the CapabilityProvider interface.
impl CapabilityProvider for HttpClientProvider {
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn Dispatcher>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        info!("Dispatcher configured");

        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;
        Ok(())
    }

    /// Handle all calls from actors.
    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn Error + Send + Sync>> {
        match op {
            OP_BIND_ACTOR if actor == SYSTEM_ACTOR => self.configure(deserialize(msg)?),
            OP_REMOVE_ACTOR if actor == SYSTEM_ACTOR => self.deconfigure(deserialize(msg)?),
            OP_REQUEST => self.request(actor, deserialize(msg)?),
            _ => Err(format!("Unknown operation: {}", op).into()),
        }
    }

    fn stop(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::deserialize;
    use mockito::mock;
    use wasmcloud_actor_http_client::{RequestArgs, Response};

    #[test]
    fn test_request() {
        let _ = env_logger::try_init();
        let request = RequestArgs {
            method: "GET".to_string(),
            url: mockito::server_url(),
            headers: HashMap::new(),
            body: vec![],
        };

        let _m = mock("GET", "/")
            .with_header("content-type", "text/plain")
            .with_body("ohai")
            .create();

        let hp = HttpClientProvider::new();
        hp.configure(CapabilityConfiguration {
            module: "test".to_string(),
            values: HashMap::new(),
        })
        .unwrap();

        let result = hp.request("test", request).unwrap();
        let response: Response = deserialize(result.as_slice()).unwrap();

        assert_eq!(response.status_code, 200);
    }
}
