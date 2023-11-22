use serde::Deserialize;
use serde_json::json;

use wasmbus_rpc::actor::prelude::*;
use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse, HttpServer, HttpServerReceiver};
use wasmcloud_interface_keyvalue::{KeyValue, KeyValueSender, SetRequest};

/// An example actor that uses the wasmcloud Smithy-based toolchain
/// to interact with the wasmcloud lattice, responding to requests over HTTP
#[derive(Debug, Default, Actor, HealthResponder)]
#[services(Actor, HttpServer)]
struct SmithyKvActor {}

/// A request body that only contains a key to use for some keyvalue operation
#[derive(Debug, Deserialize)]
struct KeyOnlyRequestBody {
    pub key: String,
}

/// Body of a HTTP request that should trigger a downstream wasmcloud:keyvalue get operation
type GetRequestBody = KeyOnlyRequestBody;

/// Body of a HTTP request that should trigger wasmcloud:keyvalue contains operation
type ContainsRequestBody = GetRequestBody;

/// Body of a HTTP request that should trigger wasmcloud:keyvalue delete operation
type DeleteRequestBody = GetRequestBody;

/// Body of a HTTP request that should trigger wasmcloud:keyvalue set_query operation
type SetQueryRequestBody = GetRequestBody;

/// Implementation of HttpServer trait methods
#[async_trait::async_trait]
impl HttpServer for SmithyKvActor {
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
            // POST /get sends a GET request via the upstream keyvalue provider
            "/get" => {
                let GetRequestBody { key } = serde_json::from_slice(&req.body).map_err(|e| {
                    RpcError::Deser(format!("failed to deserialize request body: {e}"))
                })?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({
                        "status": "success",
                        "data": KeyValueSender::new().get(ctx, &key).await?,
                    }))
                        .map_err(|e| RpcError::ActorHandler(format!("serialization failure: {e}")))?,
                    ..HttpResponse::default()
                })
            }

            // POST /set sends a SET request via the upstream keyvalue provider
            "/set" => {
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({
                        "status": "success",
                        "data": KeyValueSender::new().set(ctx, &serde_json::from_slice::<SetRequest>(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?).await?,
                    }))
                        .map_err(|e| RpcError::ActorHandler(format!("serialization failure: {e}")))?,
                    ..HttpResponse::default()
                })
            }

            // POST /contains sends a CONTAINS request via the upstream keyvalue provider
            "/contains" => {
                let ContainsRequestBody { key } = serde_json::from_slice(&req.body).map_err(|e| {
                    RpcError::Deser(format!("failed to deserialize request body: {e}"))
                })?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({
                        "status": "success",
                        "data": KeyValueSender::new().contains(ctx, &key).await?,
                    }))
                        .map_err(|e| RpcError::ActorHandler(format!("serialization failure: {e}")))?,
                    ..HttpResponse::default()
                })
            }

            // POST /list sends a LIST request via the upstream keyvalue provider
            "/set-query" => {
                let SetQueryRequestBody { key } = serde_json::from_slice(&req.body).map_err(|e| {
                    RpcError::Deser(format!("failed to deserialize request body: {e}"))
                })?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({
                        "status": "success",
                        "data": KeyValueSender::new().set_query(ctx, &key).await?,
                    }))
                        .map_err(|e| RpcError::ActorHandler(format!("serialization failure: {e}")))?,
                    ..HttpResponse::default()
                })
            }

            // POST /del sends a DEL request via the upstream keyvalue provider
            "/del" => {
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({
                        "status": "success",
                        "data": KeyValueSender::new().del(
                            ctx,
                            &serde_json::from_slice::<DeleteRequestBody>(&req.body).map_err(|e| {
                                RpcError::Deser(format!("failed to deserialize request body: {e}"))
                            })?.key
                        ).await?,
                    }))
                        .map_err(|e| RpcError::ActorHandler(format!("serialization failure: {e}")))?,
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
