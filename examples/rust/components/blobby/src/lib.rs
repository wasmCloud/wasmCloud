mod bindings {
    use crate::Blobby;

    wit_bindgen::generate!({
        with: {
            "wasi:blobstore/blobstore@0.2.0-draft": generate,
            "wasi:blobstore/container@0.2.0-draft": generate,
            "wasi:blobstore/types@0.2.0-draft": generate,
            "wasi:clocks/monotonic-clock@0.2.1": ::wasi::clocks::monotonic_clock,
            "wasi:http/incoming-handler@0.2.1": generate,
            "wasi:http/types@0.2.1": ::wasi::http::types,
            "wasi:io/error@0.2.1": ::wasi::io::error,
            "wasi:io/poll@0.2.1": ::wasi::io::poll,
            "wasi:io/streams@0.2.1": ::wasi::io::streams,
            "wasi:logging/logging@0.1.0-draft": generate,
        }
    });

    // export! defines that the `Blobby` struct defined below is going to define
    // the exports of the `world`, namely the `run` function.
    export!(Blobby);
}

use std::io::Write as _;

use http::{
    header::{ALLOW, CONTENT_LENGTH},
    StatusCode,
};

use ::wasi::http::types::*;
use ::wasi::io::streams::InputStream;
use bindings::exports::wasi::http::incoming_handler::Guest;
use bindings::wasi::blobstore;
use bindings::wasi::logging::logging::{log, Level};

struct Error {
    status_code: StatusCode,
    message: String,
}

impl Error {
    fn from_blobstore_error(e: blobstore::blobstore::Error) -> Self {
        Error {
            status_code: StatusCode::BAD_GATEWAY,
            message: format!("Error when communicating with blobstore: {}", e),
        }
    }

    fn not_found() -> Self {
        Error {
            status_code: StatusCode::NOT_FOUND,
            message: "Object not found".to_string(),
        }
    }
}

type Result<T> = std::result::Result<T, Error>;

struct Blobby;

const DEFAULT_CONTAINER_NAME: &str = "default";
// TODO: Replace functionality for setting container name via header
#[allow(dead_code)]
const CONTAINER_HEADER_NAME: &str = "blobby-container";
const CONTAINER_PARAM_NAME: &str = "container";

/// A helper that will automatically create a container if it doesn't exist and returns an owned copy of the name for immediate use
fn ensure_container(name: &String) -> Result<()> {
    if !blobstore::blobstore::container_exists(name).map_err(Error::from_blobstore_error)? {
        log(
            Level::Info,
            "handle",
            format!("creating missing container/bucket [{name}]").as_str(),
        );
        blobstore::blobstore::create_container(name).map_err(Error::from_blobstore_error)?;
    }
    Ok(())
}

fn send_response_error(response_out: ResponseOutparam, error: Error) {
    let response = OutgoingResponse::new(Fields::new());
    response
        .set_status_code(error.status_code.as_u16())
        .expect("Unable to set status code");
    let response_body = response.body().expect("body called more than once");
    let mut stream = response_body.write().expect("should only call write once");

    if let Err(e) = stream.write_all(error.message.as_bytes()) {
        log(
            Level::Error,
            "handle",
            format!("Failed to write to stream: {}", e).as_str(),
        );
        return;
    }
    // Make sure to release the write resources
    drop(stream);
    OutgoingBody::finish(response_body, None).expect("failed to finish response body");
    ResponseOutparam::set(response_out, Ok(response));
}

/// Implementation of Blobby trait methods
impl Guest for Blobby {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let path_and_query = request.path_with_query().unwrap_or_else(|| "/".to_string());

        // Derive the appropriate file and container name from the path & query string
        //
        // ex. 'localhost:8000' -> bucket name 'default', file_name ''
        // ex. 'localhost:8000?container=test' -> bucket name 'test', file name ''
        // ex. 'localhost:8000/your-file.txt?container=test' -> bucket name 'test', file name 'your-file.txt'
        let (file_name, container_id) = match path_and_query.split_once('?') {
            Some((path, query)) => {
                // We have a query string, so let's split it into a container name and a file name
                let container_id = query
                    .split('&')
                    .filter_map(|p| p.split_once('='))
                    .find(|(k, _)| *k == CONTAINER_PARAM_NAME)
                    .map(|(_, v)| v)
                    .unwrap_or(DEFAULT_CONTAINER_NAME);
                (path.trim_matches('/').to_string(), container_id.to_string())
            }
            None => (
                path_and_query.trim_matches('/').to_string(),
                DEFAULT_CONTAINER_NAME.to_string(),
            ),
        };

        // Ensure that a file name is present
        if file_name.is_empty() {
            send_response_error(
                response_out,
                Error {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Please pass a valid file (object) by specifying a URL path (ex. 'localhost:8000/some-path')".into(),
                },
            );
            return;
        }

        // Check that there isn't any sub-pathing.
        // If we can split, it means that we have more than one path element
        if file_name.split_once('/').is_some() {
            send_response_error(
                response_out,
                Error {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Cannot use a subpathed file name (e.g. foo/bar.txt)".into(),
                },
            );
            return;
        }

        // Get the container name from the request
        if let Err(e) = ensure_container(&container_id) {
            send_response_error(response_out, e);
            return;
        }

        let res = match request.method() {
            Method::Get => {
                let (data, mut size) = match get_object(&container_id, &file_name) {
                    Ok((data, size)) => (data, size),
                    Err(e) => {
                        send_response_error(response_out, e);
                        return;
                    }
                };
                let response = OutgoingResponse::new(Fields::new());
                response
                    .set_status_code(StatusCode::OK.as_u16())
                    .expect("Unable to set status code");
                let response_body = response.body().unwrap();
                ResponseOutparam::set(response_out, Ok(response));
                log(Level::Debug, "handle", "Writing data to stream");
                let stream = response_body.write().expect("failed to get stream");
                while size > 0 {
                    let len = stream
                        .blocking_splice(&data, size)
                        .expect("failed to stream blob to HTTP response body");
                    size = size.saturating_sub(len);
                }
                OutgoingBody::finish(response_body, None)
                    .map_err(|e| {
                        log(
                            Level::Error,
                            "handle",
                            format!("Failed to finish response body: {}", e).as_str(),
                        );
                        e
                    })
                    .expect("failed to finish outgoing body");

                return;
            }
            Method::Post | Method::Put => {
                // TODO: The blobstore spec doesn't have a content-type so we should probably add
                // that and start passing again
                let body = match request.consume() {
                    Ok(b) => b,
                    Err(_) => {
                        send_response_error(
                            response_out,
                            Error {
                                status_code: StatusCode::BAD_REQUEST,
                                message: "Failed to read request body".to_string(),
                            },
                        );
                        return;
                    }
                };
                // HACK(thomastaylor312): We are requiring the content length header to be set so we
                // can limit the bytes when splicing the stream.
                let raw_header = request.headers().get(&CONTENT_LENGTH.to_string());
                // Yep, wasi http really is gross. The way the `get` function works is that it
                // returns an empty vec if the header is not set, but a vec with one empty vec if
                // the header is set but empty. If either of those are the cases, we should return
                // an error
                if raw_header.first().map(|val| val.is_empty()).unwrap_or(true) {
                    send_response_error(
                        response_out,
                        Error {
                            status_code: StatusCode::BAD_REQUEST,
                            message: "Content length header is required to be set and have a value"
                                .to_string(),
                        },
                    );
                    return;
                }
                let opt_header = raw_header.first();
                // We can unwrap here because we know that the header is set and has a value
                let str_parsed_header = match std::str::from_utf8(opt_header.unwrap()) {
                    Ok(s) => s,
                    Err(e) => {
                        send_response_error(
                            response_out,
                            Error {
                                status_code: StatusCode::BAD_REQUEST,
                                message: format!("Failed to parse content length header: {}", e),
                            },
                        );
                        return;
                    }
                };
                let content_length: u64 = match str_parsed_header.parse() {
                    Ok(s) => s,
                    Err(e) => {
                        send_response_error(
                            response_out,
                            Error {
                                status_code: StatusCode::BAD_REQUEST,
                                message: format!("Failed to parse content length header: {}", e),
                            },
                        );
                        return;
                    }
                };

                let stream = body
                    .stream()
                    .expect("Unable to get stream from request body");
                put_object(&container_id, &file_name, stream, content_length)
            }
            Method::Delete => delete_object(&container_id, &file_name),
            _ => {
                let response = OutgoingResponse::new(
                    Fields::from_list(&[(
                        ALLOW.to_string(),
                        "GET,POST,PUT,DELETE".as_bytes().to_vec(),
                    )])
                    .unwrap(),
                );
                response
                    .set_status_code(StatusCode::METHOD_NOT_ALLOWED.as_u16())
                    .expect("Unable to set status code");
                ResponseOutparam::set(response_out, Ok(response));
                return;
            }
        };

        match res {
            Ok(r) => {
                let response = OutgoingResponse::new(Fields::new());
                response
                    .set_status_code(r.as_u16())
                    .expect("Unable to set status code");
                ResponseOutparam::set(response_out, Ok(response));
            }
            Err(e) => {
                send_response_error(response_out, e);
            }
        }
    }
}

// HACK(thomastaylor312): We are returning the full object in memory because there isn't really a
// way to glue in the streams to each other right now. This should get better in wasi 0.2.2 with the
// stream forward function
fn get_object(container_name: &String, object_name: &String) -> Result<(InputStream, u64)> {
    // Check that the object exists first. If it doesn't return the proper http response
    let container =
        blobstore::blobstore::get_container(container_name).map_err(Error::from_blobstore_error)?;
    if !container
        .has_object(object_name)
        .map_err(Error::from_blobstore_error)?
    {
        return Err(Error::not_found());
    }

    let metadata = container
        .object_info(object_name)
        .map_err(Error::from_blobstore_error)?;
    let incoming = container
        .get_data(object_name, 0, metadata.size)
        .map_err(Error::from_blobstore_error)?;
    let body =
        bindings::wasi::blobstore::types::IncomingValue::incoming_value_consume_async(incoming)
            .map_err(Error::from_blobstore_error)?;

    log(Level::Info, "get_object", "successfully got object stream");
    Ok((body, metadata.size))
}

fn delete_object(container_name: &String, object_name: &String) -> Result<StatusCode> {
    let container =
        blobstore::blobstore::get_container(container_name).map_err(Error::from_blobstore_error)?;

    container
        .delete_object(object_name)
        .map(|_| StatusCode::OK)
        .map_err(Error::from_blobstore_error)
}

fn put_object(
    container_name: &String,
    object_name: &String,
    data: InputStream,
    mut content_length: u64,
) -> Result<StatusCode> {
    let container =
        blobstore::blobstore::get_container(container_name).map_err(Error::from_blobstore_error)?;
    let result_value = blobstore::types::OutgoingValue::new_outgoing_value();

    let stream = result_value
        .outgoing_value_write_body()
        .expect("failed to get outgoing value output stream");

    if let Err(e) = container.write_data(object_name, &result_value) {
        log(
            Level::Error,
            "put_object",
            &format!("Failed to write data to blobstore: {}", e),
        );
        return Err(Error {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to write data to blobstore: {}", e),
        });
    }
    while content_length > 0 {
        let len = stream
            .blocking_splice(&data, content_length)
            .expect("failed to stream data from http response to blobstore");
        content_length = content_length.saturating_sub(len);
    }
    drop(stream);

    blobstore::types::OutgoingValue::finish(result_value).expect("failed to write data");

    Ok(StatusCode::CREATED)
}
