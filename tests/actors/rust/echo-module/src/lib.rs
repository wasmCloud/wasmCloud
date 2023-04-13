#![cfg(target_arch = "wasm32")]

use serde_json::json;
use wasmbus_rpc::actor::prelude::*;
use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse, HttpServer, HttpServerReceiver};

#[derive(Debug, Default, Actor, HealthResponder)]
#[services(Actor, HttpServer)]
struct Echo;

#[async_trait]
impl HttpServer for Echo {
    async fn handle_request(
        &self,
        _: &Context,
        HttpRequest {
            method,
            path,
            query_string,
            body,
            header,
        }: &HttpRequest,
    ) -> RpcResult<HttpResponse> {
        let body = serde_json::to_vec(&json!({
            "method": method,
            "path": path,
            "query_string": query_string,
            "body": body,
            "header": header,
        }))
        .map_err(|e| RpcError::ActorHandler(format!("failed to serialize response: {e}")))?;
        Ok(HttpResponse {
            body,
            ..Default::default()
        })
    }
}
