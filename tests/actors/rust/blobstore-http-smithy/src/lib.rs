use serde::Deserialize;
use serde_json::json;

use wasmbus_rpc::actor::prelude::*;
use wasmcloud_interface_blobstore::{Blobstore, BlobstoreSender};
use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse, HttpServer, HttpServerReceiver};

/// An example actor that uses the wasmcloud Smithy-based toolchain
/// to interact with the wasmcloud lattice, responding to requests over HTTP
#[derive(Debug, Default, Actor, HealthResponder)]
#[services(Actor, HttpServer)]
struct SmithyKvActor {}

/// Body of a HTTP request that should trigger a downstream wasmcloud:blobstore create-container operation
#[derive(Debug, Deserialize)]
struct CreateContainerRequest {
    pub name: String,
}

/// Body of a HTTP request that should trigger a downstream wasmcloud:blobstore remove-containers operation
#[derive(Debug, Deserialize)]
struct RemoveContainersRequest {
    pub names: Vec<String>,
}

/// Body of a HTTP request that should trigger a downstream wasmcloud:blobstore container-exists operation
#[derive(Debug, Deserialize)]
struct ContainerExistsRequest {
    pub name: String,
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
            "/create-container" => {
                let CreateContainerRequest { name } =
                    serde_json::from_slice(&req.body).map_err(|e| {
                        RpcError::Deser(format!("failed to deserialize request body: {e}"))
                    })?;
                let data = BlobstoreSender::new().create_container(ctx, &name).await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/container-exists" => {
                let ContainerExistsRequest { name } =
                    serde_json::from_slice(&req.body).map_err(|e| {
                        RpcError::Deser(format!("failed to deserialize request body: {e}"))
                    })?;
                let data = BlobstoreSender::new().container_exists(ctx, &name).await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            "/remove-containers" => {
                let RemoveContainersRequest { names } =
                    serde_json::from_slice(&req.body).map_err(|e| {
                        RpcError::Deser(format!("failed to deserialize request body: {e}"))
                    })?;
                let data = BlobstoreSender::new()
                    .remove_containers(ctx, &names)
                    .await?;
                Ok(HttpResponse {
                    body: serde_json::to_vec(&json!({ "status": "success", "data": data }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                    ..HttpResponse::default()
                })
            }

            path if path.starts_with("/put-object") => {
                // Extract intended container and object from URL path
                let [_, _, ref container_id, ref object_id] = path
                    .split('/')
                    .map(ToString::to_string)
                    .collect::<Vec<String>>()[..]
                else {
                    return Ok(HttpResponse {
                        body: serde_json::to_vec(&json!({
                            "status": "error",
                            "msg": format!("invalid request path [{path}]"),
                        }))
                        .map_err(|e| {
                            RpcError::ActorHandler(format!("serialization failure: {e}"))
                        })?,
                        ..HttpResponse::default()
                    });
                };

                let data = BlobstoreSender::new()
                    .put_object(
                        ctx,
                        &wasmcloud_interface_blobstore::PutObjectRequest {
                            chunk: wasmcloud_interface_blobstore::Chunk {
                                object_id: object_id.into(),
                                container_id: container_id.into(),
                                bytes: req.body.clone(),
                                offset: 0,
                                is_last: true,
                            },
                            content_type: None,
                            content_encoding: None,
                        },
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

            "/object-exists" => {
                let data = BlobstoreSender::new()
                    .object_exists(
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

            "/list-objects" => {
                let data = BlobstoreSender::new()
                    .list_objects(
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

            "/remove-objects" => {
                let data = BlobstoreSender::new()
                    .remove_objects(
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

            "/get-object" => {
                let data = BlobstoreSender::new()
                    .get_object(
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
