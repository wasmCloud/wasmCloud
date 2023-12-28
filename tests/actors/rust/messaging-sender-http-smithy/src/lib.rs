use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use wasmbus_rpc::actor::prelude::*;
use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse, HttpServer, HttpServerReceiver};
use wasmcloud_interface_messaging::{Messaging, MessagingSender, PubMessage, RequestMessage};

/// An example actor that uses the wasmcloud Smithy-based toolchain
/// to interact with the wasmcloud lattice, responding to requests over HTTP
#[derive(Debug, Default, Actor, HealthResponder)]
#[services(Actor, HttpServer)]
struct SmithyMessagingSenderActor {}

/// Body of a HTTP request that should trigger a downstream wasmcloud:messaging publish operation
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishRequest {
    pub msg: PubMessage,
}

/// Body of a HTTP request that should trigger a downstream wasmcloud:messaging request operation
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestRequest {
    pub msg: RequestMessage,
}

/// Implementation of HttpServer trait methods
#[async_trait]
impl HttpServer for SmithyMessagingSenderActor {
    async fn handle_request(&self, ctx: &Context, req: &HttpRequest) -> RpcResult<HttpResponse> {
        // Only allow HTTP POST requests
        let HttpRequest { method, .. } = req;
        if &req.method.to_lowercase() != "post" {
            return Ok(HttpResponse {
                status_code: 400,
                body: serde_json::to_vec(&json!({
                    "status": "error",
                    "msg": format!("invalid request method [{method}], all requests must be POST"),
                }))
                .map_err(|e| RpcError::ActorHandler(format!("serialization failure: {e}")))?,
                ..HttpResponse::default()
            });
        }

        // Build response by performing the given operation
        match req.path.as_ref() {
            "/publish" => {
                let PublishRequest { msg } = serde_json::from_slice(&req.body).map_err(|e| {
                    RpcError::Deser(format!("failed to deserialize request body: {e}"))
                })?;
                let data = MessagingSender::new().publish(ctx, &msg).await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/request" => {
                let RequestRequest { msg } = serde_json::from_slice(&req.body).map_err(|e| {
                    RpcError::Deser(format!("failed to deserialize request body: {e}"))
                })?;
                let data = MessagingSender::new().request(ctx, &msg).await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            _ => Ok(HttpResponse {
                status_code: 400,
                body: serde_json::to_vec(&json!({
                    "status": "error",
                    "msg": "invalid request, unrecognized path",
                }))
                .map_err(|e| RpcError::ActorHandler(format!("serialization failure: {e}")))?,
                ..HttpResponse::default()
            }),
        }
    }
}
