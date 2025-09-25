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

const DEFAULT_CONTAINER_NAME: &str = "default";
const CONTAINER_PARAM_NAME: &str = "container";

/// Implementation of Blobby trait methods
impl http::Server for Blobby {
    fn handle(
        request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        let (parts, body) = request.into_parts();
        let path_and_query = parts.uri.path_and_query().map(|pq| (pq.path(), pq.query()));

        // Derive the appropriate file and container name from the path & query string
        //
        // ex. 'localhost:8000' -> bucket name 'default', file_name ''
        // ex. 'localhost:8000?container=test' -> bucket name 'test', file name ''
        // ex. 'localhost:8000/your-file.txt?container=test' -> bucket name 'test', file name 'your-file.txt'
        let (file_name, container_id) = match path_and_query {
            Some((path, Some(query))) => {
                // We have a query string, so let's split it into a container name and a file name
                let container_id = query
                    .split('&')
                    .filter_map(|p| p.split_once('='))
                    .find(|(k, _)| *k == CONTAINER_PARAM_NAME)
                    .map(|(_, v)| v)
                    .unwrap_or(DEFAULT_CONTAINER_NAME);
                (path.trim_matches('/').to_string(), container_id.to_string())
            }
            Some((path, None)) => (
                path.trim_matches('/').to_string(),
                DEFAULT_CONTAINER_NAME.to_string(),
            ),
            // Ensure that a file name is present
            None => {
                return http::Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(ResponseBody::from(
                        "Please pass a valid file (object) by specifying a URL path",
                    ))
                    .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string())))
            }
        };

        // Check that there isn't any sub-pathing.
        // If we can split, it means that we have more than one path element
        if file_name.split_once('/').is_some() {
            return http::Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body("Cannot use a subpathed file name (e.g. foo/bar.txt)".into())
                .map_err(|e| http::ErrorCode::InternalError(Some(e.to_string())));
        }

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
        Err(
            "Expected to copy {content_length} bytes, but only copied {copied_bytes} bytes"
                .to_string(),
        )
    } else {
        OutgoingValue::finish(result_value).map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// Delete the object from the container. This function is idempotent.
fn delete_object(container: Container, object_name: &String) -> Result<(), String> {
    container.delete_object(object_name)
}

http::export!(Blobby);
