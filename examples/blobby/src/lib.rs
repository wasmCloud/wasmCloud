use std::num::ParseIntError;

use ::http::{
    header::{ALLOW, CONTENT_LENGTH},
    Method, StatusCode,
};
use wasmcloud_component::{
    debug, error, http, info,
    wasi::blobstore::blobstore,
    wasi::blobstore::{
        blobstore::Container,
        types::{IncomingValue, OutgoingValue},
    },
    wasi::io::streams::InputStream,
};

static UI_HTML: &str = include_str!("../ui.html");

/// Helper enum and associated set of `impl`s to allow for returning either a simple
/// `String` or a `InputStream` as the response body.
enum ResponseBody {
    Ok(InputStream),
    Err(String),
}
impl From<&str> for ResponseBody {
    fn from(s: &str) -> Self {
        ResponseBody::Err(s.to_string())
    }
}
impl From<String> for ResponseBody {
    fn from(s: String) -> Self {
        ResponseBody::Err(s)
    }
}
impl From<InputStream> for ResponseBody {
    fn from(s: InputStream) -> Self {
        ResponseBody::Ok(s)
    }
}
impl http::OutgoingBody for ResponseBody {
    fn write(
        self,
        body: wasmcloud_component::wasi::http::types::OutgoingBody,
        stream: wasmcloud_component::wasi::io::streams::OutputStream,
    ) -> std::io::Result<()> {
        match self {
            ResponseBody::Ok(data) => InputStream::write(data, body, stream),
            ResponseBody::Err(e) => String::write(e, body, stream),
        }
    }
}

struct Blobby;

const DEFAULT_CONTAINER_NAME: &str = "blobby";

/// Implementation of Blobby trait methods
impl http::Server for Blobby {
    fn handle(
        request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        let container_id = DEFAULT_CONTAINER_NAME.to_string();
        
        // Ensure the container exists
        let container = match ensure_container(&container_id) {
            Ok(container) => container,
            Err(e) => {
                return http::Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(e.into())
                    .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string())));
            }
        };

        let (parts, body) = request.into_parts();

        let path = match parts.uri.path_and_query() {
            Some(path) => path.path().trim_matches('/'),
            None => "",
        };

        // Handle UI related request
        if path == "" {
            match parts.method {
                // UI HTML
                Method::GET => {
                    return http::Response::builder()
                        .status(StatusCode::OK)
                        .body(ResponseBody::from(UI_HTML))
                        .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string())));
                }
                // File List
                Method::POST=> {
                    let mut js_list = Vec::<String>::new();

                    let object_list = container.list_objects().map_err(|e| http::ErrorCode::InternalError(Some(e.to_string())))?;
                    while let Ok((list_result, stream_end)) = object_list.read_stream_object_names(100) {
                        js_list.extend(list_result);
                         
                        if stream_end{
                            break
                        }
                    }

                    js_list.sort();

                    let js_response = serde_json::to_string_pretty(&js_list).map_err(|e| http::ErrorCode::InternalError(Some(e.to_string())))?;

                    return http::Response::builder()
                        .status(StatusCode::OK)
                        .body(ResponseBody::from(js_response))
                        .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string())));
                }
                _ => {
                    return http::Response::builder()
                        .status(StatusCode::METHOD_NOT_ALLOWED)
                        .body(ResponseBody::from("Method not allowed"))
                        .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string())));
                }
            }
        }

        let file_name = path.to_string();
        
        match parts.method {
            Method::GET => {
                let (data, _size) = match get_object(container, &file_name) {
                    Ok((data, size)) => (data, size),
                    Err(e) => {
                        return http::Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .body(e.into())
                            .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string())));
                    }
                };
                debug!("streaming response data");
                http::Response::builder()
                    .status(StatusCode::OK)
                    .body(data.into())
                    .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string())))
            }
            Method::POST | Method::PUT => match parts.headers.get(CONTENT_LENGTH.to_string()) {
                None => http::Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body("Content length header is required to be set and have a value".into())
                    .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string()))),

                Some(raw_header) if raw_header.is_empty() => http::Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body("Content length header is required to have a value".into())
                    .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string()))),
                Some(raw_header) => {
                    let content_length: u64 = match std::str::from_utf8(raw_header.as_bytes())
                        .map(|h| h.parse().map_err(|e: ParseIntError| e.to_string()))
                        .map_err(|e| e.to_string())
                    {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) | Err(e) => {
                            return http::Response::builder()
                                .status(StatusCode::BAD_REQUEST)
                                .body(format!("Failed to parse content length header: {e}").into())
                                .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string())));
                        }
                    };

                    match put_object(container, &file_name, body, content_length) {
                        Ok(_) => http::Response::builder()
                            .status(StatusCode::CREATED)
                            .body(format!("Wrote {file_name} to blobstore successfully").into())
                            .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string()))),
                        Err(e) => http::Response::builder()
                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                            .body(e.into())
                            .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string()))),
                    }
                }
            },
            Method::DELETE => match delete_object(container, &file_name) {
                Ok(_) => http::Response::builder()
                    .status(StatusCode::OK)
                    .body(format!("Deleted {file_name} from blobstore successfully").into())
                    .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string()))),
                Err(e) => http::Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(e.into())
                    .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string()))),
            },
            _ => http::Response::builder()
                .status(StatusCode::METHOD_NOT_ALLOWED)
                .header(ALLOW, "GET,POST,PUT,DELETE")
                .body("Method not allowed".into())
                .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string()))),
        }
    }
}

/// Helper to fetch a container if it exists, otherwise create it
fn ensure_container(name: &String) -> Result<Container, String> {
    if !blobstore::container_exists(name)? {
        info!("creating missing container/bucket [{name}]");
        blobstore::create_container(name)
    } else {
        debug!("container/bucket [{name}] already exists");
        blobstore::get_container(name)
    }
}

/// Get the object data from the container
fn get_object(container: Container, object_name: &String) -> Result<(InputStream, u64), String> {
    // Check that the object exists first. If it doesn't return the proper http response
    if !container.has_object(object_name)? {
        return Err("Object not found".to_string());
    }

    let metadata = container.object_info(object_name)?;
    let incoming = container.get_data(object_name, 0, metadata.size)?;
    let body = IncomingValue::incoming_value_consume_async(incoming)?;
    debug!("successfully got object stream");
    Ok((body, metadata.size))
}

/// Write the object data to the object in the container
fn put_object(
    container: Container,
    object_name: &String,
    mut data: http::IncomingBody,
    content_length: u64,
) -> Result<(), String> {
    let result_value = OutgoingValue::new_outgoing_value();
    let mut stream = result_value
        .outgoing_value_write_body()
        .expect("failed to get outgoing value output stream");
    if let Err(e) = container.write_data(object_name, &result_value) {
        error!("Failed to write data to blobstore: {e}");
        return Err(format!("Failed to write data to blobstore: {e}"));
    }

    let copied_bytes = std::io::copy(&mut data, &mut stream).map_err(|e| e.to_string())?;
    if copied_bytes != content_length {
        Err(format!(
            "Expected to copy {} bytes, but only copied {} bytes",
            content_length, copied_bytes
        ))
    } else {
        // Flush before drop: ensures any buffered bytes are committed to the underlying
        // pipe before we drop the stream handle. With MemoryOutputPipe this is a no-op,
        // but it's correct defensive practice for any OutputStream implementation.
        stream.flush().map_err(|e| e.to_string())?;
        drop(stream);
        OutgoingValue::finish(result_value).map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Delete the object from the container. This function is idempotent.
fn delete_object(container: Container, object_name: &String) -> Result<(), String> {
    container.delete_object(object_name)
}

http::export!(Blobby);
