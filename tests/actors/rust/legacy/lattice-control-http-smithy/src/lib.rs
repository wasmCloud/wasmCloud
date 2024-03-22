use serde::Deserialize;
use serde_json::json;

use wasmbus_rpc::actor::prelude::*;
use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse, HttpServer, HttpServerReceiver};
use wasmcloud_interface_lattice_control::{LatticeController, LatticeControllerSender};

/// An example actor that uses the wasmcloud Smithy-based toolchain
/// to interact with the wasmcloud lattice, responding to requests over HTTP
#[derive(Debug, Default, Actor, HealthResponder)]
#[services(Actor, HttpServer)]
struct SmithyKvActor {}

/// Body of a HTTP request that should trigger a downstream wasmcloud:latticecontrol get-hosts operation
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetHostsRequest {
    pub lattice_id: String,
}

/// Body of a HTTP request that should trigger a downstream wasmcloud:latticecontrol get-claims operation
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetClaimsRequest {
    pub lattice_id: String,
}

/// Body of a HTTP request that should trigger a downstream wasmcloud:latticecontrol get-links operation
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetLinksRequest {
    pub lattice_id: String,
}

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
            "/get-hosts" => {
                let GetHostsRequest { lattice_id } =
                    serde_json::from_slice(&req.body).map_err(|e| {
                        RpcError::Deser(format!("failed to deserialize request body: {e}"))
                    })?;
                let data = LatticeControllerSender::new()
                    .get_hosts(ctx, &lattice_id)
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/get-claims" => {
                let GetClaimsRequest { lattice_id } =
                    serde_json::from_slice(&req.body).map_err(|e| {
                        RpcError::Deser(format!("failed to deserialize request body: {e}"))
                    })?;
                let data = LatticeControllerSender::new()
                    .get_claims(ctx, &lattice_id)
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/get-host-inventory" => {
                let data = LatticeControllerSender::new()
                    .get_host_inventory(
                        ctx,
                        &serde_json::from_slice(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?,
                    )
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/get-links" => {
                let GetLinksRequest { lattice_id } =
                    serde_json::from_slice(&req.body).map_err(|e| {
                        RpcError::Deser(format!("failed to deserialize request body: {e}"))
                    })?;
                let data = LatticeControllerSender::new()
                    .get_links(ctx, &lattice_id)
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/auction-provider" => {
                let data = LatticeControllerSender::new()
                    .auction_provider(
                        ctx,
                        &serde_json::from_slice(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?,
                    )
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/auction-actor" => {
                let data = LatticeControllerSender::new()
                    .auction_actor(
                        ctx,
                        &serde_json::from_slice(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?,
                    )
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/advertise-link" => {
                let data = LatticeControllerSender::new()
                    .advertise_link(
                        ctx,
                        &serde_json::from_slice(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?,
                    )
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/remove-link" => {
                let data = LatticeControllerSender::new()
                    .remove_link(
                        ctx,
                        &serde_json::from_slice(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?,
                    )
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/scale-actor" => {
                let data = LatticeControllerSender::new()
                    .scale_actor(
                        ctx,
                        &serde_json::from_slice(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?,
                    )
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/update-actor" => {
                let data = LatticeControllerSender::new()
                    .update_actor(
                        ctx,
                        &serde_json::from_slice(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?,
                    )
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/start-provider" => {
                let data = LatticeControllerSender::new()
                    .start_provider(
                        ctx,
                        &serde_json::from_slice(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?,
                    )
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/stop-provider" => {
                let data = LatticeControllerSender::new()
                    .stop_provider(
                        ctx,
                        &serde_json::from_slice(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?,
                    )
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/stop-host" => {
                let data = LatticeControllerSender::new()
                    .stop_host(
                        ctx,
                        &serde_json::from_slice(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?,
                    )
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/set-registry-credentials" => {
                let data = LatticeControllerSender::new()
                    .set_registry_credentials(
                        ctx,
                        &serde_json::from_slice(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?,
                    )
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/set-lattice-credentials" => {
                let data = LatticeControllerSender::new()
                    .set_lattice_credentials(
                        ctx,
                        &serde_json::from_slice(&req.body).map_err(|e| {
                            RpcError::Deser(format!("failed to deserialize request body: {e}"))
                        })?,
                    )
                    .await?;
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
