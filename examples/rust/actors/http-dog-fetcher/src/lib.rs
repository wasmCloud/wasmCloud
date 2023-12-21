wit_bindgen::generate!({
    world: "hello",
    exports: {
        "wasi:http/incoming-handler": HttpServer,
    },
    with: {
        "wasi:io/streams@0.2.0-rc-2023-11-10": wasmcloud_actor::wasi::io::streams,
    }
});

mod wasi_helpers;

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::*;

use wasmcloud_actor::info;
use wasmcloud_actor::wasi::blobstore::*;

struct HttpServer;

/// HTTP response from https://random.dog/woof.json, just deserializing the URL
#[derive(serde::Deserialize)]
struct RandomDog {
    url: String,
}

impl Guest for HttpServer {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        info!("Handling dog fetcher request");

        let response = crate::wasi_helpers::http::outgoing_http_request(
            Scheme::Https,
            "random.dog",
            "/woof.json",
        )
        .expect("failed to make HTTP request for random dog");
        let response_body = response
            .consume()
            .expect("failed to retrieve response body")
            .stream()
            .expect("failed to retrieve response body stream")
            // The response should be a very small JSON payload, so this magic number is acceptable
            .blocking_read(wasi_helpers::io::STREAM_READ_BUFFER_SIZE)
            .expect("failed to read response body stream");
        let random_dog: RandomDog = serde_json::from_slice(&response_body).expect(
            format!(
                "failed to parse response body as JSON: {:?}",
                String::from_utf8_lossy(&response_body)
            )
            .as_str(),
        );

        info!("Fetching dog image from {}", random_dog.url);
        let path = random_dog.url.trim_start_matches("https://random.dog");
        let dog_image_resp =
            crate::wasi_helpers::http::outgoing_http_request(Scheme::Https, "random.dog", path)
                .expect("failed to make HTTP request for dog image");
        let name = "dogs".to_string();
        let container = if blobstore::container_exists(&name).unwrap() {
            blobstore::get_container(&name).unwrap()
        } else {
            blobstore::create_container(&name).unwrap()
        };

        let cons = dog_image_resp
            .consume()
            .expect("failed to consume dog image response");
        let input_stream = cons
            .stream()
            .expect("failed to get dog image response stream");

        info!("Writing dog image to {}", path);
        input_stream.subscribe().block();
        let object_name = path.trim_start_matches('/').to_string();
        let object_handle = types::new_outgoing_value();
        let output_stream = types::outgoing_value_write_body(object_handle).unwrap();
        crate::wasi_helpers::io::connect_streams(input_stream, output_stream);
        container::write_data(container, &object_name, object_handle)
            .expect("failed to write object to blobstore");

        let response = OutgoingResponse::new(Fields::new());
        let response_body = response.body().unwrap();
        response_body
            .write()
            .expect("failed to write to response body")
            .blocking_write_and_flush(format!("Dog image saved to {object_name}\n").as_bytes())
            .expect("failed to block write and flush response body");
        OutgoingBody::finish(response_body, None).expect("failed to finish response body");
        ResponseOutparam::set(response_out, Ok(response));
    }
}
