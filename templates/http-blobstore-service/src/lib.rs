mod bindings {
    wit_bindgen::generate!({
        world: "http-blobstore-service",
        path: "wit",
        with: {
            "wasi:io/error@0.2.1": ::wasip2::io::error,
            "wasi:io/poll@0.2.1": ::wasip2::io::poll,
            "wasi:io/streams@0.2.1": ::wasip2::io::streams,
        },
        generate_all,
    });
}

use axum::{
    Json, Router,
    body::Bytes,
    extract::Path,
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use bindings::wasi::blobstore::{
    blobstore,
    container::Container,
    types::{IncomingValue, OutgoingValue},
};
use serde::Serialize;
use wstd::io::{AsyncInputStream, AsyncOutputStream};

const CONTAINER_NAME: &str = "default";

#[wstd_axum::http_server]
fn main() -> Router {
    Router::new()
        .route("/", get(list_objects))
        .route("/{key}", get(get_object).put(put_object).delete(delete_object))
        .fallback(not_found)
}

#[derive(Serialize)]
struct ObjectList {
    container: String,
    keys: Vec<String>,
}

async fn list_objects() -> Result<Json<ObjectList>, AppError> {
    let container = ensure_container(CONTAINER_NAME)?;
    let stream = container
        .list_objects()
        .map_err(|e| AppError::internal(format!("list_objects: {e}")))?;
    let mut keys = Vec::new();
    loop {
        let batch = stream
            .read_stream_object_names(64)
            .map_err(|e| AppError::internal(format!("read_stream_object_names: {e}")))?;
        if batch.0.is_empty() {
            break;
        }
        keys.extend(batch.0);
        if batch.1 {
            break;
        }
    }
    Ok(Json(ObjectList {
        container: CONTAINER_NAME.to_string(),
        keys,
    }))
}

async fn get_object(Path(key): Path<String>) -> Result<Vec<u8>, AppError> {
    let container = ensure_container(CONTAINER_NAME)?;
    if !container
        .has_object(&key)
        .map_err(|e| AppError::internal(format!("has_object: {e}")))?
    {
        return Err(AppError::not_found(key));
    }
    let size = container
        .object_info(&key)
        .map_err(|e| AppError::internal(format!("object_info: {e}")))?
        .size;
    let incoming = container
        .get_data(&key, 0, size)
        .map_err(|e| AppError::internal(format!("get_data: {e}")))?;
    let stream = IncomingValue::incoming_value_consume_async(incoming)
        .map_err(|e| AppError::internal(format!("consume_async: {e}")))?;
    let mut buf = Vec::with_capacity(size as usize);
    let mut input = AsyncInputStream::new(stream);
    use wstd::io::AsyncRead;
    input
        .read_to_end(&mut buf)
        .await
        .map_err(|e| AppError::internal(format!("read_to_end: {e}")))?;
    Ok(buf)
}

async fn put_object(Path(key): Path<String>, body: Bytes) -> Result<StatusCode, AppError> {
    let container = ensure_container(CONTAINER_NAME)?;
    let outgoing = OutgoingValue::new_outgoing_value();
    let stream = outgoing
        .outgoing_value_write_body()
        .map_err(|_| AppError::internal("outgoing_value_write_body"))?;
    let output = AsyncOutputStream::new(stream);
    output
        .write_all(&body)
        .await
        .map_err(|e| AppError::internal(format!("write_all: {e}")))?;
    drop(output);
    container
        .write_data(&key, &outgoing)
        .map_err(|e| AppError::internal(format!("write_data: {e}")))?;
    OutgoingValue::finish(outgoing)
        .map_err(|e| AppError::internal(format!("finish: {e}")))?;
    Ok(StatusCode::CREATED)
}

async fn delete_object(Path(key): Path<String>) -> Result<StatusCode, AppError> {
    let container = ensure_container(CONTAINER_NAME)?;
    container
        .delete_object(&key)
        .map_err(|e| AppError::internal(format!("delete_object: {e}")))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn not_found() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "Not found\n")
}

fn ensure_container(name: &str) -> Result<Container, AppError> {
    if blobstore::container_exists(&name.to_string())
        .map_err(|e| AppError::internal(format!("container_exists: {e}")))?
    {
        blobstore::get_container(&name.to_string())
            .map_err(|e| AppError::internal(format!("get_container: {e}")))
    } else {
        blobstore::create_container(&name.to_string())
            .map_err(|e| AppError::internal(format!("create_container: {e}")))
    }
}

struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }

    fn not_found(key: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: format!("key '{key}' not found"),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (self.status, format!("{}\n", self.message)).into_response()
    }
}
