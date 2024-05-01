wit_bindgen::generate!({
    world: "hello",
    exports: {
        "wasi:http/incoming-handler": BlobstoreComponent,
    },
});

use wasi::blobstore::blobstore::{container_exists, create_container, get_container};
use wasi::http::types::*;
use wasi::logging::logging::{log, Level};

const CONTAINER_NAME: &str = "foo";

struct BlobstoreComponent;

impl exports::wasi::http::incoming_handler::Guest for BlobstoreComponent {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        let container = if container_exists(&CONTAINER_NAME.to_string()).unwrap() {
            log(Level::Debug, "", "Container already exists, fetching ...");
            get_container(&CONTAINER_NAME.to_string()).expect("container to exist")
        } else {
            log(Level::Info, "", "Container did not exist, creating ...");
            create_container(&CONTAINER_NAME.to_string()).expect("to be able to create container")
        };
        let (all_objects, _end_of_stream) = container
            .list_objects()
            .unwrap()
            .read_stream_object_names(999)
            .unwrap();

        // Send back HTTP request
        let response = OutgoingResponse::new(Fields::new());
        let response_body = response.body().expect("response body to exist");
        response_body
            .write()
            .unwrap()
            .blocking_write_and_flush(
                format!(
                    "There are {} objects in the {CONTAINER_NAME} container\n",
                    all_objects.len()
                )
                .as_bytes(),
            )
            .unwrap();
        OutgoingBody::finish(response_body, None).expect("failed to finish response body");
        ResponseOutparam::set(response_out, Ok(response));
    }
}
