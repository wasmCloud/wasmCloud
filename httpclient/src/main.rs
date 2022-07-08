//! Httpclient capability provider
//!
//! This implementation is multi-threaded and requests from different actors
//! use different connections and can run in parallel.
use std::str::FromStr;

use reqwest::header as http;
use reqwest::header::HeaderMap as HttpHeaderMap;
use tracing::{error, instrument, trace, warn};
use wasmbus_rpc::provider::prelude::*;
use wasmcloud_interface_httpclient::{HttpClient, HttpClientReceiver, HttpRequest, HttpResponse};

// main (via provider_main) initializes the threaded tokio executor,
// listens to lattice rpcs, handles actor links,
// and returns only when it receives a shutdown message
//
fn main() -> Result<(), Box<dyn std::error::Error>> {
    provider_main(
        HttpClientProvider::default(),
        Some("HttpClient Provider".to_string()),
    )?;

    eprintln!("HttpClient provider exiting");
    Ok(())
}

/// HTTP client capability provider implementation
#[derive(Default, Clone, Provider)]
#[services(HttpClient)]
struct HttpClientProvider {}

/// use default implementations of provider message handlers
impl ProviderDispatch for HttpClientProvider {}
/// we don't need to override put_link, delete_link, or shutdown
impl ProviderHandler for HttpClientProvider {}

/// Handle HttpClient methods
#[async_trait]
impl HttpClient for HttpClientProvider {
    /// Accepts a request from an actor and forwards it to a remote http server.
    /// This function returns an RpcError if there was a network-related
    /// error sending the request. If the remote server returned an http
    /// error (status other than 2xx), returns Ok with the status code and
    /// body returned from the remote server.
    #[instrument(level = "debug", skip(self, _ctx, req), fields(actor_id = ?_ctx.actor, method = %req.method, url = %req.url))]
    async fn request(&self, _ctx: &Context, req: &HttpRequest) -> RpcResult<HttpResponse> {
        let mut headers: HttpHeaderMap = HttpHeaderMap::default();
        convert_request_headers(&req.headers, &mut headers);
        let body = req.body.to_vec();
        let method = reqwest::Method::from_str(&req.method)
            .map_err(|e| RpcError::InvalidParameter(format!("method: {}:{}", &req.method, e)))?;
        trace!("forwarding {} request to {}", &req.method, &req.url);
        let response = reqwest::Client::new()
            .request(method, &req.url)
            .headers(headers)
            .body(body)
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
                RpcError::Other(format!("sending request: {}", e))
            })?;

        let headers = convert_response_headers(response.headers());
        let status_code = response.status().as_u16();
        let body = response.bytes().await.map_err(|e|
                // error receiving the body could occur if the connection was
                // closed before it was fully received
                RpcError::Other(format!("receiving response body: {}", e)))?;
        if (200..300).contains(&(status_code as usize)) {
            trace!(
                %status_code,
                "http request completed",
            );
        } else {
            warn!(
                %status_code,
                "http request completed with non-200 status"
            );
        }
        Ok(HttpResponse {
            body: body.to_vec(),
            header: headers,
            status_code,
        })
    }
}

/// convert response headers from reqwest to HeaderMap
fn convert_response_headers(
    headers: &reqwest::header::HeaderMap,
) -> wasmcloud_interface_httpclient::HeaderMap {
    let mut hmap = wasmcloud_interface_httpclient::HeaderMap::new();
    for k in headers.keys() {
        let vals = headers
            .get_all(k)
            .iter()
            // from http crate:
            //    In practice, HTTP header field values are usually valid ASCII.
            //     However, the HTTP spec allows for a header value to contain
            //     opaque bytes as well.
            // This implementation only forwards headers with ascii values to the actor.
            .filter_map(|val| val.to_str().ok())
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        if !vals.is_empty() {
            hmap.insert(k.to_string(), vals);
        }
    }
    hmap
}

/// convert HeaderMap from actor into outgoing HeaderMap
fn convert_request_headers(
    header: &wasmcloud_interface_httpclient::HeaderMap,
    headers_mut: &mut http::HeaderMap,
) {
    let map = headers_mut;
    for (k, vals) in header.iter() {
        let name = match http::HeaderName::from_bytes(k.as_bytes()) {
            Ok(name) => name,
            Err(e) => {
                error!(
                    error = %e,
                    header_name = %k,
                    "invalid response header name, sending without this header"
                );
                continue;
            }
        };
        for val in vals.iter() {
            let value = match http::HeaderValue::from_str(val) {
                Ok(value) => value,
                Err(e) => {
                    error!(
                        error = %e,
                        header_value = %val,
                        "Non-ascii header value, skipping this header",
                    );
                    continue;
                }
            };
            map.append(&name, value);
        }
    }
}
