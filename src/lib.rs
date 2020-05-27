///!
///! # http-client-provider
///! This library exposes the HTTP client capability to waSCC-compliant actors

mod http_client;

#[macro_use]
extern crate wascc_codec as codec;

#[macro_use]
extern crate log;

use codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use codec::core::{CapabilityConfiguration, OP_BIND_ACTOR, OP_REMOVE_ACTOR};
use codec::deserialize;
use codec::http::{Request, OP_PERFORM_REQUEST};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::sync::RwLock;

const CAPABILITY_ID: &str = "wascc:http_client";
const SYSTEM_ACTOR: &str = "system";

#[cfg(not(feature = "static_plugin"))]
capability_provider!(HttpClientProvider, HttpClientProvider::new);

/// An implementation HTTP client provider using reqwest.
pub struct HttpClientProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    clients: Arc<RwLock<HashMap<String, reqwest::Client>>>,
    runtime: tokio::runtime::Runtime,
}

impl HttpClientProvider {
    /// Create a new HTTP client provider.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the HTTP client for a particular actor.
    /// Each actor gets a dedicated client so that we can take advantage of connection pooling.
    /// TODO: This needs to set things like timeouts, redirects, etc.
    fn configure(&self, config: CapabilityConfiguration) -> Result<Vec<u8>, Box<dyn Error>> {
        // TODO read in config and set defaults
        self.clients
            .write()
            .unwrap()
            .insert(config.module.clone(), reqwest::Client::new());
        Ok(vec![])
    }

    /// Clean up resources when a actor disconnects.
    /// This removes the HTTP client associated with an actor.
    fn deconfigure(&self, config: CapabilityConfiguration) -> Result<Vec<u8>, Box<dyn Error>> {
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
    fn request(&self, actor: &str, msg: Request) -> Result<Vec<u8>, Box<dyn Error>> {
        let lock = self.clients.read().unwrap();
        let client = lock.get(actor).unwrap();
        self.runtime
            .handle()
            .block_on(async { http_client::request(&client, msg).await })
    }
}

impl Default for HttpClientProvider {
    fn default() -> Self {
        let _ = env_logger::builder().format_module_path(false).try_init();

        let r = tokio::runtime::Builder::new()
            .threaded_scheduler()
            .enable_all()
            .build()
            .unwrap();

        HttpClientProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            clients: Arc::new(RwLock::new(HashMap::new())),
            runtime: r,
        }
    }
}

/// Implements the CapabilityProvider interface.
impl CapabilityProvider for HttpClientProvider {
    fn configure_dispatch(&self, dispatcher: Box<dyn Dispatcher>) -> Result<(), Box<dyn Error>> {
        info!("Dispatcher configured");

        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;
        Ok(())
    }

    /// The name of the provider.
    fn name(&self) -> &'static str {
        "waSCC Default HTTP Client"
    }

    /// The capability ID provided by the implementation.
    fn capability_id(&self) -> &'static str {
        CAPABILITY_ID
    }

    /// Handle all calls from actors.
    fn handle_call(&self, actor: &str, op: &str, msg: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
        match op {
            OP_BIND_ACTOR if actor == SYSTEM_ACTOR => self.configure(deserialize(msg)?),
            OP_REMOVE_ACTOR if actor == SYSTEM_ACTOR => self.deconfigure(deserialize(msg)?),
            OP_PERFORM_REQUEST => self.request(actor, deserialize(msg)?),
            _ => Err(format!("Unknown operation: {}", op).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codec::deserialize;
    use codec::http::Response;
    use mockito::mock;

    #[test]
    fn test_request() {
        let _ = env_logger::try_init();
        let request = Request {
            method: "GET".to_string(),
            path: mockito::server_url(),
            header: HashMap::new(),
            body: vec![],
            query_string: String::new(),
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
