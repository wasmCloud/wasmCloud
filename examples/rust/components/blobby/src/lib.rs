#![allow(clippy::missing_safety_doc)]
// ^^^ This is for clippy complaining about something that wit bindgen is generating

wit_bindgen::generate!({
    world: "blobby",
});

use std::io::{Read, Write};

use http::{
    header::{ALLOW, CONTENT_LENGTH},
    StatusCode,
};

use exports::wasi::http::incoming_handler::Guest;
use wasi::blobstore::blobstore;
use wasi::http::types::*;
use wasi::logging::logging::{log, Level};
use wrapper::OutputStreamWriter;

mod wrapper;

struct Error {
    status_code: StatusCode,
    message: String,
}

impl Error {
    fn from_blobstore_error(e: blobstore::Error) -> Self {
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
    if !blobstore::container_exists(name).map_err(Error::from_blobstore_error)? {
        blobstore::create_container(name).map_err(Error::from_blobstore_error)?;
    }
    Ok(())
}

fn send_response_error(response_out: ResponseOutparam, error: Error) {
    let response = OutgoingResponse::new(Fields::new());
    response
        .set_status_code(error.status_code.as_u16())
        .expect("Unable to set status code");
    let response_body = response.body().expect("body called more than once");
    let mut writer = response_body.write().expect("should only call write once");

    let mut stream = OutputStreamWriter::from(&mut writer);

    if let Err(e) = stream.write_all(error.message.as_bytes()) {
        log(
            Level::Error,
            "handle",
            format!("Failed to write to stream: {}", e).as_str(),
        );
        return;
    }
    // Make sure to release the write resources
    drop(writer);
    OutgoingBody::finish(response_body, None).expect("failed to finish response body");
    ResponseOutparam::set(response_out, Ok(response));
}

/// Implementation of Blobby trait methods
impl Guest for Blobby {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let path_and_query = request.path_with_query().unwrap_or_else(|| "/".to_string());

        let (file_name, container_id) = match path_and_query.split_once('?') {
            Some((path, query)) => {
                // We have a query string, so let's split it into a container name and a file name
                let mut params = query.split('&');
                let container_id = params
                    .find(|p| *p == CONTAINER_PARAM_NAME)
                    .and_then(|p| p.split_once('=').map(|(_, val)| val))
                    .unwrap_or(DEFAULT_CONTAINER_NAME);
                (path.trim_matches('/').to_string(), container_id.to_string())
            }
            None => (
                path_and_query.trim_matches('/').to_string(),
                DEFAULT_CONTAINER_NAME.to_string(),
            ),
        };

        // Check that there isn't any subpathing. If we can split, it means that we have more than
        // one path element
        if file_name.split_once('/').is_some() {
            send_response_error(
                response_out,
                Error {
                    status_code: StatusCode::BAD_REQUEST,
                    message: "Cannot use a subpathed file name (e.g. foo/bar.txt)".to_string(),
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
                let data = match get_object(&container_id, &file_name) {
                    Ok(s) => s,
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
                let mut stream = response_body.write().expect("Unable to get stream");
                let mut outstream = OutputStreamWriter::from(&mut stream);
                if let Err(e) = outstream.write_all(&data) {
                    log(
                        Level::Error,
                        "handle",
                        format!("Failed to write to stream: {}", e).as_str(),
                    );
                    send_response_error(
                        response_out,
                        Error {
                            status_code: StatusCode::INTERNAL_SERVER_ERROR,
                            message: "Unable to send data to client".to_string(),
                        },
                    );
                    return;
                }
                if let Err(e) = outstream.flush() {
                    log(
                        Level::Error,
                        "handle",
                        format!("Failed to flush stream: {}", e).as_str(),
                    );
                    send_response_error(
                        response_out,
                        Error {
                            status_code: StatusCode::INTERNAL_SERVER_ERROR,
                            message: "Unable to flush data to client".to_string(),
                        },
                    );
                    return;
                }
                // This MUST be dropped to free the resource
                drop(stream);
                let response = OutgoingBody::finish(response_body, None)
                    .map(|_| response)
                    .map_err(|e| {
                        log(
                            Level::Error,
                            "handle",
                            format!("Failed to finish response body: {}", e).as_str(),
                        );
                        e
                    });
                ResponseOutparam::set(response_out, response);
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
                let mut stream = body
                    .stream()
                    .expect("Unable to get stream from request body");
                // HACK(thomastaylor312): We are requiring the content length header to be set so we
                // can splice the stream properly. This should be better with wasi 0.2.2 which will
                // introduce a stream forward function
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

                // Only read up to the exact amount of bytes we need. This is to prevent a bad actor
                // from sending infinite data
                let mut buf = vec![
                    0u8;
                    content_length
                        .try_into()
                        .expect("Too much data to read into actor")
                ];
                if let Err(e) = stream.read_exact(&mut buf) {
                    log(
                        Level::Error,
                        "handle",
                        format!("Failed to read request body: {}", e).as_str(),
                    );
                    send_response_error(
                        response_out,
                        Error {
                            status_code: StatusCode::BAD_REQUEST,
                            message: "Failed to read request body".to_string(),
                        },
                    );
                    return;
                }

                put_object(&container_id, &file_name, buf)
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
fn get_object(container_name: &String, object_name: &String) -> Result<Vec<u8>> {
    // Check that the object exists first. If it doesn't return the proper http response
    let container =
        blobstore::get_container(container_name).map_err(Error::from_blobstore_error)?;
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
    let body = wasi::blobstore::types::IncomingValue::incoming_value_consume_sync(incoming)
        .map_err(Error::from_blobstore_error)?;

    log(Level::Info, "get_object", "successfully got object stream");
    Ok(body)
}

fn delete_object(container_name: &String, object_name: &String) -> Result<StatusCode> {
    let container =
        blobstore::get_container(container_name).map_err(Error::from_blobstore_error)?;

    container
        .delete_object(object_name)
        .map(|_| StatusCode::OK)
        .map_err(Error::from_blobstore_error)
}

// HACK(thomastaylor312): We are passing the full object in memory because there isn't really a way
// to glue in the streams to each other right now. This should get better in wasi 0.2.2 with the
// stream forward function
fn put_object(container_name: &String, object_name: &String, data: Vec<u8>) -> Result<StatusCode> {
    let container =
        blobstore::get_container(container_name).map_err(Error::from_blobstore_error)?;
    let result_value = wasi::blobstore::types::OutgoingValue::new_outgoing_value();

    let mut body = result_value
        .outgoing_value_write_body()
        .expect("failed to get outgoing value output stream");

    let mut out = OutputStreamWriter::from(&mut body);

    if let Err(e) = out.write_all(&data) {
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

    if let Err(e) = out.flush() {
        log(
            Level::Error,
            "put_object",
            &format!("Failed to flush data to blobstore: {}", e),
        );
        return Err(Error {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to flush data to blobstore: {}", e),
        });
    }

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

    Ok(StatusCode::CREATED)
}

// export! defines that the `Blobby` struct defined below is going to define
// the exports of the `world`, namely the `run` function.
export!(Blobby);
