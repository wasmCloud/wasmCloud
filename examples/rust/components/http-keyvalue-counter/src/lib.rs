wit_bindgen::generate!({
    world: "hello",
});

use exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::*;

struct HttpServer;

impl Guest for HttpServer {
    fn handle(request: IncomingRequest, response_out: ResponseOutparam) {
        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let response_body = response.body().unwrap();

        let name = request
            .path_with_query()
            .expect("failed to get path with query");

        let bucket =
            wasi::keyvalue::types::Bucket::open_bucket("foo").expect("failed to open empty bucket");
        let count = wasi::keyvalue::atomic::increment(&bucket, &name, 1)
            .expect("failed to increment count");

        response_body
            .write()
            .unwrap()
            .blocking_write_and_flush(format!("Counter {name}: {count}\n").as_bytes())
            .unwrap();
        OutgoingBody::finish(response_body, None).expect("failed to finish response body");
        ResponseOutparam::set(response_out, Ok(response));
    }
}

export!(HttpServer);
