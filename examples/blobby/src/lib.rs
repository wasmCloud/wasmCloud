mod bindings {
    wit_bindgen::generate!({
        world: "blobby",
        path: "wit",
        with: {
            "wasi:io/error@0.2.1": ::wasip2::io::error,
            "wasi:io/poll@0.2.1": ::wasip2::io::poll,
            "wasi:io/streams@0.2.1": ::wasip2::io::streams,
        },
        generate_all,
    });
}

use bindings::wasi::blobstore::{
    blobstore,
    container::Container,
    types::{IncomingValue, OutgoingValue},
};
use wstd::http::{Body, Method, Request, Response, StatusCode};
use wstd::io::{AsyncInputStream, AsyncOutputStream};

static UI_HTML: &str = include_str!("../ui.html");
const DEFAULT_CONTAINER_NAME: &str = "blobby";

#[wstd::http_server]
async fn main(request: Request<Body>) -> Result<Response<Body>, wstd::http::Error> {
    let container = ensure_container(DEFAULT_CONTAINER_NAME)?;

    let (parts, mut body) = request.into_parts();

    let path = parts
        .uri
        .path_and_query()
        .map(|p| p.path().trim_matches('/'))
        .unwrap_or("");

    if path.is_empty() {
        return match parts.method {
            Method::GET => Ok(Response::new(UI_HTML.into())),
            Method::POST => {
                let list = list_objects(&container)?;
                let json = serde_json::to_string_pretty(&list)
                    .map_err(|e| wstd::http::Error::msg(e.to_string()))?;
                Ok(Response::new(json.into()))
            }
            _ => Ok(Response::builder()
                .status(StatusCode::METHOD_NOT_ALLOWED)
                .body("Method not allowed".into())
                .unwrap()),
        };
    }

    let file_name = path.to_string();

    match parts.method {
        Method::GET => match get_object(&container, &file_name) {
            Ok(stream) => Ok(Response::new(Body::from(stream))),
            Err(e) => Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(e.into())
                .unwrap()),
        },
        Method::POST | Method::PUT => {
            let content_length: u64 = parts
                .headers
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| wstd::http::Error::msg("Content-Length header is required"))?;

            match put_object(&container, &file_name, &mut body, content_length).await {
                Ok(_) => Ok(Response::builder()
                    .status(StatusCode::CREATED)
                    .body(format!("Wrote {file_name} to blobstore successfully").into())
                    .unwrap()),
                Err(e) => Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(e.into())
                    .unwrap()),
            }
        }
        Method::DELETE => match container.delete_object(&file_name) {
            Ok(_) => Ok(Response::new(
                format!("Deleted {file_name} from blobstore successfully").into(),
            )),
            Err(e) => Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(e.into())
                .unwrap()),
        },
        _ => Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body("Method not allowed".into())
            .unwrap()),
    }
}

fn ensure_container(name: &str) -> Result<Container, wstd::http::Error> {
    if !blobstore::container_exists(name).map_err(|e| wstd::http::Error::msg(e))? {
        blobstore::create_container(name).map_err(|e| wstd::http::Error::msg(e))
    } else {
        blobstore::get_container(name).map_err(|e| wstd::http::Error::msg(e))
    }
}

fn list_objects(container: &Container) -> Result<Vec<String>, wstd::http::Error> {
    let mut names = Vec::new();
    let object_list = container
        .list_objects()
        .map_err(|e| wstd::http::Error::msg(e))?;

    while let Ok((list, end)) = object_list.read_stream_object_names(100) {
        names.extend(list);
        if end {
            break;
        }
    }

    names.sort();
    Ok(names)
}

fn get_object(container: &Container, object_name: &str) -> Result<AsyncInputStream, String> {
    if !container.has_object(object_name)? {
        return Err("Object not found".to_string());
    }

    let metadata = container.object_info(object_name)?;
    let incoming = container.get_data(object_name, 0, metadata.size)?;
    let stream = IncomingValue::incoming_value_consume_async(incoming)?;
    Ok(AsyncInputStream::new(stream))
}

async fn put_object(
    container: &Container,
    object_name: &str,
    body: &mut Body,
    content_length: u64,
) -> Result<(), String> {
    let outgoing_value = OutgoingValue::new_outgoing_value();
    let output_stream = outgoing_value
        .outgoing_value_write_body()
        .map_err(|_| "failed to get outgoing value output stream".to_string())?;

    container
        .write_data(object_name, &outgoing_value)
        .map_err(|e| format!("Failed to write data to blobstore: {e}"))?;

    let async_stream = AsyncOutputStream::new(output_stream);
    let data = body
        .contents()
        .await
        .map_err(|e| format!("Failed to read request body: {e}"))?;

    if data.len() as u64 != content_length {
        return Err(format!(
            "Expected to copy {content_length} bytes, but got {} bytes",
            data.len()
        ));
    }

    async_stream
        .write_all(data)
        .await
        .map_err(|e| format!("Failed to write to output stream: {e}"))?;
    async_stream
        .flush()
        .await
        .map_err(|e| format!("Failed to flush output stream: {e}"))?;

    // Stream must be dropped before calling finish
    drop(async_stream);

    OutgoingValue::finish(outgoing_value)
        .map_err(|e| format!("Failed to finish outgoing value: {e}"))?;

    Ok(())
}
