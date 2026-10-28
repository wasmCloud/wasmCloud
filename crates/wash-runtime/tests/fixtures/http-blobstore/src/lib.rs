use wasmcloud_component::{
    http,
    wasi::blobstore::{
        container::IncomingValue,
        types::{InputStream, OutgoingValue},
    },
};

struct Component;

const CONTAINER: &str = "my-container-real";
const OBJECT: &str = "hi.txt";

enum ResponseBody {
    String(String),
    Stream(InputStream),
}

/// Answers a blobstore operation: 200 with its result, or 500 with its error.
fn operation_response(
    result: Result<String, String>,
) -> http::Result<http::Response<ResponseBody>> {
    let (status, body) = match result {
        Ok(body) => (200, body),
        Err(e) => (500, e),
    };
    Ok(http::Response::builder()
        .status(status)
        .body(ResponseBody::String(body))
        .expect("failed to create HTTP body for operation response"))
}

/// The names of the container's objects, sorted and comma-separated.
fn list_names(
    container: &wasmcloud_component::wasi::blobstore::container::Container,
) -> Result<String, String> {
    let stream = container
        .list_objects()
        .map_err(|e| format!("list_objects: {e:?}"))?;
    let mut names = Vec::new();
    loop {
        let (mut batch, done) = stream
            .read_stream_object_names(16)
            .map_err(|e| format!("read_stream_object_names: {e:?}"))?;
        names.append(&mut batch);
        if done {
            break;
        }
    }
    names.sort();
    Ok(names.join(","))
}

impl http::OutgoingBody for ResponseBody {
    fn write(
        self,
        body: wasmcloud_component::wasi::http::types::OutgoingBody,
        stream: wasmcloud_component::wasi::io::streams::OutputStream,
    ) -> std::io::Result<()> {
        match self {
            ResponseBody::String(s) => s.write(body, stream),
            ResponseBody::Stream(data) => InputStream::write(data, body, stream),
        }
    }
}

impl http::Server for Component {
    fn handle(
        request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        let container = match wasmcloud_component::wasi::blobstore::blobstore::get_container(
            &String::from(CONTAINER),
        ) {
            Ok(c) => c,
            Err(_) => match wasmcloud_component::wasi::blobstore::blobstore::create_container(
                &String::from(CONTAINER),
            ) {
                Ok(c) => c,
                Err(e) => {
                    return Ok(http::Response::builder()
                        .status(500)
                        .body(ResponseBody::String(format!(
                            "failed to create container: {:?}",
                            e
                        )))
                        .expect("failed to create HTTP body for container error"));
                }
            },
        };

        let (parts, mut body) = request.into_parts();

        match parts.uri.path() {
            "/op/has-object" => {
                return operation_response(
                    container
                        .has_object(&String::from(OBJECT))
                        .map(|found| found.to_string())
                        .map_err(|e| format!("has_object: {e:?}")),
                );
            }
            "/op/object-info" => {
                return operation_response(
                    container
                        .object_info(&String::from(OBJECT))
                        .map(|info| format!("{} {} {}", info.name, info.size, info.container))
                        .map_err(|e| format!("object_info: {e:?}")),
                );
            }
            "/op/list" => return operation_response(list_names(&container)),
            _ => {}
        }

        let outgoing_value = OutgoingValue::new_outgoing_value();
        let mut output_stream = match outgoing_value.outgoing_value_write_body() {
            Ok(s) => s,
            Err(e) => {
                return Ok(http::Response::builder()
                    .status(500)
                    .body(ResponseBody::String(format!(
                        "failed to create output stream: {:?}",
                        e
                    )))
                    .expect("failed to create HTTP body for output stream error"));
            }
        };

        if let Err(e) = container.write_data(&String::from(OBJECT), &outgoing_value) {
            return Ok(http::Response::builder()
                .status(500)
                .body(ResponseBody::String(format!(
                    "failed to write data to container: {:?}",
                    e
                )))
                .expect("failed to create HTTP body for write data error"));
        }

        if let Err(e) = std::io::copy(&mut body, &mut output_stream) {
            return Ok(http::Response::builder()
                .status(500)
                .body(ResponseBody::String(format!(
                    "failed to write to output stream: {:?}",
                    e
                )))
                .expect("failed to create HTTP body for write to output stream error"));
        }

        if let Err(e) = OutgoingValue::finish(outgoing_value) {
            return Ok(http::Response::builder()
                .status(500)
                .body(ResponseBody::String(format!(
                    "failed to finish writing file: {:?}",
                    e
                )))
                .expect("failed to create HTTP body for finish writing file error"));
        }

        let data = match container.get_data(&String::from(OBJECT), 0, u64::MAX) {
            Ok(d) => d,
            Err(e) => {
                return Ok(http::Response::builder()
                    .status(500)
                    .body(ResponseBody::String(format!(
                        "failed to read data from container: {:?}",
                        e
                    )))
                    .expect("failed to create HTTP body for read data error"));
            }
        };

        let stream = match IncomingValue::incoming_value_consume_async(data) {
            Ok(s) => s,
            Err(e) => {
                return Ok(http::Response::builder()
                    .status(500)
                    .body(ResponseBody::String(format!(
                        "failed to consume incoming value: {:?}",
                        e
                    )))
                    .expect("failed to create HTTP body for consume incoming value error"));
            }
        };

        Ok(http::Response::new(ResponseBody::Stream(stream)))
    }
}

http::export!(Component);
