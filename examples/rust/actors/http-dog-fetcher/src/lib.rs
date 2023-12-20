wit_bindgen::generate!({
    world: "hello",
    exports: {
        "wasi:http/incoming-handler": HttpServer,
    },
});

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::*;

use wasmcloud_actor::info;
use wasmcloud_actor::wasi::blobstore::*;

struct HttpServer;

impl Guest for HttpServer {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        info!("Handling dog fetcher request");

        let request = OutgoingRequest::new(Fields::new());
        request
            .set_authority(Some("random.dog"))
            .expect("failed to set authority");
        request
            .set_path_with_query(Some("/woof.json"))
            .expect("failed to set path with query");
        request
            .set_scheme(Some(&Scheme::Https))
            .expect("failed to set scheme");
        let request_body = request.body().expect("request body not found");
        OutgoingBody::finish(request_body, None).expect("failed to finish sending request body");

        let response = wasi::http::outgoing_handler::handle(request, None)
            .expect("failed to handle HTTP request");
        response.subscribe().block();

        let random_dog: RandomDog = serde_json::from_slice(
            &response
                .get()
                .unwrap()
                .unwrap()
                .unwrap()
                .consume()
                .unwrap()
                .stream()
                .unwrap()
                .blocking_read(999999)
                .unwrap(),
        )
        .expect("failed to parse JSON response");

        info!("Fetching dog image from {}", random_dog.url);
        let request = OutgoingRequest::new(Fields::new());
        request
            .set_authority(Some("random.dog"))
            .expect("failed to set authority");
        let path = random_dog.url.trim_start_matches("https://random.dog");
        request
            .set_path_with_query(Some(path))
            .expect("failed to set path with query");
        request
            .set_scheme(Some(&Scheme::Https))
            .expect("failed to set scheme");
        let request_body = request.body().expect("request body not found");
        OutgoingBody::finish(request_body, None).expect("failed to finish sending request body");

        let response = wasi::http::outgoing_handler::handle(request, None)
            .expect("failed to handle HTTP request");
        response.subscribe().block();
        let resp = response.get().unwrap().unwrap().unwrap();
        let cons = resp.consume().unwrap();
        let stream = cons.stream().unwrap();

        let name = "dawgs".to_string();
        let container = if blobstore::container_exists(&name).unwrap() {
            blobstore::get_container(&name).unwrap()
        } else {
            blobstore::create_container(&name).unwrap()
        };

        info!("Writing dog image to {}", path);
        stream.subscribe().block();
        let object_name = path.trim_start_matches('/').to_string();
        let outgoing_value = types::new_outgoing_value();
        let outgoing_stream = types::outgoing_value_write_body(outgoing_value).unwrap();
        while let Ok(buf) = stream.read(4096) {
            if buf.is_empty() {
                break;
            }
            outgoing_stream.blocking_write_and_flush(&buf).unwrap();
        }
        container::write_data(container, &object_name, outgoing_value).unwrap();

        let response = OutgoingResponse::new(Fields::new());
        let response_body = response.body().unwrap();
        response_body
            .write()
            .expect("failed to write to response body")
            .blocking_write_and_flush(format!("Dog image saved to {object_name}").as_bytes())
            .expect("failed to block write and flush response body");
        OutgoingBody::finish(response_body, None).expect("failed to finish response body");
        ResponseOutparam::set(response_out, Ok(response));
    }
}

#[derive(serde::Deserialize)]
struct RandomDog {
    url: String,
}
