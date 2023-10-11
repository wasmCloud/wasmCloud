use std::collections::HashMap;
use std::str::FromStr;

use async_trait::async_trait;
use http::{HeaderMap, HeaderName, HeaderValue};
use tracing::{error, instrument, trace, warn};
use wasmcloud_provider_sdk::{
    core::LinkDefinition,
    error::{ProviderInvocationError, ProviderInvocationResult},
    Context,
};

wasmcloud_provider_wit_bindgen::generate!({
    impl_struct: HttpClientProvider,
    contract: "wasmcloud:httpclient",
    replace_witified_maps: true,
    wit_bindgen_cfg: "provider-http-client"
});

/// HTTP client capability provider implementation struct
#[derive(Default, Clone)]
pub struct HttpClientProvider;

/// Implement the [httpclient contract](https://github.com/wasmCloud/interfaces/blob/main/httpclient)
/// represented by the WIT interface @ `wit/provider-httpclient.wit`
#[async_trait]
impl WasmcloudHttpclientHttpClient for HttpClientProvider {
    /// Accepts a request from an actor and forwards it to a remote http server.
    ///
    /// This function returns an RpcError if there was a network-related
    /// error sending the request. If the remote server returned an http
    /// error (status other than 2xx), returns Ok with the status code and
    /// body returned from the remote server.
    #[instrument(level = "debug", skip(self, _ctx, req), fields(actor_id = ?_ctx.actor, method = %req.method, url = %req.url))]
    async fn request(
        &self,
        _ctx: Context,
        req: HttpRequest,
    ) -> ProviderInvocationResult<HttpResponse> {
        let headers: HeaderMap = build_http_header_map(&req.headers)?;

        let method = reqwest::Method::from_str(&req.method).map_err(|e| {
            ProviderInvocationError::Provider(format!(
                "failed to convert method: {}:{e}",
                &req.method
            ))
        })?;

        trace!("forwarding {} request to {}", &req.method, &req.url);
        // Perform request to upstream server that was requested by the actor
        let response = reqwest::Client::new()
            .request(method, &req.url)
            .headers(headers)
            .body(req.body)
            .send()
            .await
            .map_err(|e| {
                // send() can fail if there was an error while sending request,
                // a redirect loop was detected, or redirect limit was exhausted.
                // For now, we'll return an error (not HttpResponse with error
                // status) and the caller should receive an error
                // (needs to be tested).
                error!(
                    error = %e,
                    "httpclient network error attempting to send"
                );
                ProviderInvocationError::Provider(format!("failed to send request: {e}"))
            })?;

        // Read information from the upstream server response to send back to the actor
        let resp_status_code = response.status().as_u16();
        let resp_headers = convert_header_map_to_hashmap(response.headers());
        let resp_body = response
            .bytes()
            .await
            // error receiving the body could occur if the connection was closed before it was fully received
            .map_err(|e| {
                ProviderInvocationError::Provider(format!(
                    "failed reading response body bytes: {e}"
                ))
            })?;

        // Log request status
        if (200..300).contains(&(resp_status_code as usize)) {
            trace!(
                %resp_status_code,
                "http request completed",
            );
        } else {
            warn!(
                %resp_status_code,
                "http request completed with non-200 status"
            );
        }

        Ok(HttpResponse {
            body: Vec::from(resp_body),
            header: resp_headers,
            status_code: resp_status_code,
        })
    }
}

/// Handle provider control commands
#[async_trait]
impl WasmcloudCapabilityProvider for HttpClientProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip(self, ld), fields(actor_id = %ld.actor_id))]
    async fn put_link(&self, ld: &LinkDefinition) -> bool {
        // Accept all links that are put without saving any information
        true
    }

    /// Handle notification that a link is dropped - close the connection
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, actor_id: &str) {
        // Deleting links is a no-op since no link information was saved
    }

    /// Handle shutdown request by closing all connections
    #[instrument(level = "debug", skip(self))]
    async fn shutdown(&self) {
        // Shutting down is a no-op since no link information was saved
    }
}

/// Build a [`http::Headermap`] from the [`std::collections::HashMap`]
/// that an incoming WIT HTTP request would produce
fn build_http_header_map(
    input: &HashMap<String, Vec<String>>,
) -> ProviderInvocationResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (k, v) in input.iter() {
        headers.append(
            HeaderName::from_str(k.as_str()).map_err(|e| {
                ProviderInvocationError::Provider(format!("failed to convert header name: {e}"))
            })?,
            // Multiple values in a header string should be joined by comma
            HeaderValue::from_str(&v.join(",")).map_err(|e| {
                ProviderInvocationError::Provider(format!("failed to convert header value: {e}"))
            })?,
        );
    }
    Ok(headers)
}

/// Convert a [`http::HeaderMap`] to a HashMap of the kind that is used in the smithy contract
fn convert_header_map_to_hashmap(map: &HeaderMap) -> HashMap<String, Vec<String>> {
    map.iter().fold(HashMap::new(), |mut headers, (k, v)| {
        headers.entry(k.to_string()).or_default().extend(
            String::from_utf8_lossy(v.as_bytes())
                // Multiple values for a given header should be separated by ','
                // https://www.rfc-editor.org/rfc/rfc9110.html#name-field-lines-and-combined-fi
                .split(',')
                .map(String::from)
                .collect::<Vec<String>>(),
        );
        headers
    })
}
