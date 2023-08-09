use std::string::ToString;

use serde::{Deserialize, Serialize};
use wasmcloud_provider_sdk::{
    core::{LinkDefinition, WasmCloudEntity},
    error::ProviderInvocationError,
};

/// map data structure for holding http headers
///
pub type HeaderMap = std::collections::HashMap<String, HeaderValues>;
pub type HeaderValues = Vec<String>;

const HANDLE_REQUEST_METHOD: &str = "HttpServer.HandleRequest";

/// HttpRequest contains data sent to actor about the http request
#[derive(Serialize, Deserialize, Debug)]
pub struct HttpRequest {
    /// HTTP method. One of: GET,POST,PUT,DELETE,HEAD,OPTIONS,CONNECT,PATCH,TRACE
    #[serde(default)]
    pub method: String,
    /// full request path
    #[serde(default)]
    pub path: String,
    /// query string. May be an empty string if there were no query parameters.
    #[serde(rename = "queryString")]
    #[serde(default)]
    pub query_string: String,
    /// map of request headers (string key, string value)
    pub header: HeaderMap,
    /// Request body as a byte array. May be empty.
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
}

/// HttpResponse contains the actor's response to return to the http client
#[derive(Serialize, Deserialize, Debug)]
pub struct HttpResponse {
    /// statusCode is a three-digit number, usually in the range 100-599,
    /// A value of 200 indicates success.
    #[serde(rename = "statusCode")]
    #[serde(default)]
    pub status_code: u16,
    /// Map of headers (string keys, list of values)
    pub header: HeaderMap,
    /// Body of response as a byte array. May be an empty array.
    #[serde(with = "serde_bytes")]
    #[serde(default)]
    pub body: Vec<u8>,
}

pub struct HttpServer<'a> {
    ld: &'a LinkDefinition,
    timeout: Option<std::time::Duration>,
}

impl<'a> HttpServer<'a> {
    pub fn new(ld: &'a LinkDefinition, timeout: Option<std::time::Duration>) -> Self {
        Self { ld, timeout }
    }

    pub async fn handle_request(
        &self,
        req: HttpRequest,
    ) -> Result<HttpResponse, ProviderInvocationError> {
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

        let response: HttpResponse = wasmcloud_provider_sdk::deserialize(&response.msg)?;

        Ok(response)
    }
}
