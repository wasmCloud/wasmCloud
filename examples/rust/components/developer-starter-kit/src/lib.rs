use std::io::Write;

mod bindings {
    wit_bindgen::generate!({
        with: {
            "wasmcloud:messaging/types@0.2.0": wasmcloud_component::wasmcloud::messaging::types
        },
        generate_all
    });
    use super::StarterKit;
    export!(StarterKit);
}

use wasmcloud_component::info;
use wasmcloud_component::wasi::blobstore;
use wasmcloud_component::wasi::exports::http::incoming_handler;
use wasmcloud_component::wasi::http::types::*;
use wasmcloud_component::wasi::keyvalue;
use wasmcloud_component::wasmcloud::messaging;
use wasmcloud_component::wasmcloud::messaging::types::BrokerMessage;

mod helpers;

struct StarterKit;

impl incoming_handler::Guest for StarterKit {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        info!(
            "Received request {:?} {:?}",
            request.method(),
            request.path_with_query()
        );
        // Using the incoming `wasi:http` request, use a `match` for basic routing
        match (request.method(), request.path_with_query().as_deref()) {
            // If the path_with_query is absent, return a 400 Bad Request
            (_, None) => {
                let response = OutgoingResponse::new(Fields::new());
                response
                    .set_status_code(400)
                    .expect("failed to set outgoing HTTP request");
                ResponseOutparam::set(response_out, Ok(response));
            }
            // GET / or GET /hello will return a 200 OK with a simple message
            (Method::Get, Some("/") | Some("/hello")) => {
                helpers::send_http_response(response_out, 200, b"Hello from Starter Kit!\n");
            }
            // GET /counter will increment a counter in a keyvalue store and return the current value
            (Method::Get, Some("/counter")) => {
                let bucket = keyvalue::store::open("default").expect("failed to open empty bucket");
                let count = keyvalue::atomics::increment(&bucket, "counter", 2)
                    .expect("failed to increment count");
                helpers::send_http_response(response_out, 200, format!("Counter: {count}\n"));
            }
            // GET /example_dot_com will make an outgoing HTTP request to example.com
            (Method::Get, Some("/example_dot_com")) => {
                let request = OutgoingRequest::new(Fields::new());
                request
                    .set_authority(Some("example.com"))
                    .expect("failed to set authority");
                request
                    .set_path_with_query(Some("/"))
                    .expect("failed to set path");
                request
                    .set_scheme(Some(&Scheme::Https))
                    .expect("failed to set scheme");
                request
                    .set_method(&Method::Get)
                    .expect("failed to set method");
                let outgoing_request = wasi::http::outgoing_handler::handle(request, None);
                let response = outgoing_request.expect("failed to make outgoing HTTP request");
                response.subscribe().block();
                let response_body = response
                    .get()
                    .expect("failed to get response body")
                    .expect("failed to get response body")
                    .expect("failed to get response body");
                let consumed = response_body
                    .consume()
                    .expect("failed to consume response body");
                let response = OutgoingResponse::new(Fields::new());
                response
                    .set_status_code(200)
                    .expect("failed to set outgoing HTTP request");
                let response_body = response.body().expect("failed to get outgoing body");
                ResponseOutparam::set(response_out, Ok(response));
                helpers::splice(
                    &consumed
                        .stream()
                        .expect("failed to get response body stream"),
                    response_body
                        .write()
                        .expect("failed to write to response body"),
                )
                .expect("failed to splice request body to response body");
                OutgoingBody::finish(response_body, None).expect("failed to finish response body");
            }
            // POST /echo will return a 200 OK with the request body streamed back
            (Method::Post, Some("/echo")) => {
                let response = OutgoingResponse::new(request.headers());
                response
                    .set_status_code(200)
                    .expect("failed to set outgoing HTTP request");
                let request_body = request.consume().expect("failed to get incoming body");
                let incoming_body_stream = request_body
                    .stream()
                    .expect("failed to get incoming body stream");
                let response_body = response.body().expect("failed to get outgoing body");
                ResponseOutparam::set(response_out, Ok(response));
                let outgoing_body_stream = response_body
                    .write()
                    .expect("failed to write to outgoing body");
                helpers::splice(&incoming_body_stream, outgoing_body_stream)
                    .expect("failed to splice request body to response body");
                OutgoingBody::finish(response_body, None).expect("failed to finish response body");
            }
            // POST /file will stream the request body to a file and return a 200 OK
            (Method::Post, Some("/file")) => {
                let response = OutgoingResponse::new(request.headers());
                let request_body = request.consume().expect("failed to get incoming body");
                let incoming_body_stream = request_body
                    .stream()
                    .expect("failed to get incoming body stream");
                let response_body = response.body().expect("failed to get outgoing body");

                let container_name = "starter_kit".to_string();
                let object_name = "streaming_data".to_string();
                let container =
                    if blobstore::blobstore::container_exists(&container_name).is_ok_and(|b| b) {
                        blobstore::blobstore::get_container(&container_name)
                    } else {
                        blobstore::blobstore::create_container(&container_name)
                    };

                if let Ok(container) = container {
                    let data = blobstore::types::OutgoingValue::new_outgoing_value();
                    let outgoing_stream = data
                        .outgoing_value_write_body()
                        .expect("failed to get outgoing value body");
                    container
                        .write_data(&object_name, &data)
                        .expect("failed to write data");
                    response
                        .set_status_code(200)
                        .expect("failed to set outgoing HTTP request");
                    ResponseOutparam::set(response_out, Ok(response));
                    response_body
                        .write()
                        .expect("failed to write to outgoing body")
                        .write_fmt(format_args!(
                            "Successfully created container {container_name} with object {object_name}\n"
                        ))
                        .expect("failed to write to outgoing body");
                    helpers::splice(&incoming_body_stream, outgoing_stream)
                        .expect("failed to splice request body to response body");
                } else {
                    response
                        .set_status_code(500)
                        .expect("failed to set outgoing HTTP request");
                    ResponseOutparam::set(response_out, Ok(response));
                    response_body
                        .write()
                        .expect("failed to write to response body")
                        .write_fmt(format_args!(
                            "Failed to create container {container_name}\n"
                        ))
                        .expect("failed to write to response body");
                }
                OutgoingBody::finish(response_body, None).expect("failed to finish response body");
            }
            _ => {
                helpers::send_http_response(response_out, 404, b"not found");
            }
        }
    }
}

// You can also implement exports as declared in your WIT world, 'wit/world.wit'
impl bindings::exports::wasmcloud::messaging::handler::Guest for StarterKit {
    fn handle_message(msg: BrokerMessage) -> Result<(), String> {
        info!("Received message {:?}", msg);
        if let Some(reply_to) = msg.reply_to {
            let reply_message = BrokerMessage {
                subject: reply_to,
                body: msg.body,
                reply_to: None,
            };
            messaging::consumer::publish(&reply_message).expect("Failed to publish reply message");
        }
        Ok(())
    }
}

wasmcloud_component::wasi::http::proxy::export!(StarterKit);
