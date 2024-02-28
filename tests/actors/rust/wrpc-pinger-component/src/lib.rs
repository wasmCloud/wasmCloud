wit_bindgen::generate!({
    world: "actor",
    exports: {
        "wasmcloud:testing/invoke": Actor,
    },
    with: {
        "wasi:io/streams@0.2.0": wasmcloud_actor::wasi::io::streams,
    }
});

use std::io::{Read, Write};

use wasi::http;
use wasi::io::poll::poll;
use wasmcloud::testing::*;
use wasmcloud_actor::{InputStreamReader, OutputStreamWriter};

struct Actor;

impl exports::wasmcloud::testing::invoke::Guest for Actor {
    fn call() -> String {
        let http_request = http::types::OutgoingRequest::new(http::types::Fields::new());
        http_request
            .set_method(&http::types::Method::Put)
            .expect("failed to set request method");
        http_request
            .set_path_with_query(Some("/test"))
            .expect("failed to set request path with query");
        http_request
            .set_scheme(Some(&http::types::Scheme::Https))
            .expect("failed to set request scheme");
        http_request
            .set_authority(Some("localhost:4242"))
            .expect("failed to set request authority");
        let http_request_body = http_request
            .body()
            .expect("failed to get outgoing request body");
        {
            let mut stream = http_request_body
                .write()
                .expect("failed to get outgoing request stream");
            let mut w = OutputStreamWriter::from(&mut stream);
            w.write_all(b"test")
                .expect("failed to write `test` to outgoing request stream");
            w.flush().expect("failed to flush outgoing request stream");
        }
        http::types::OutgoingBody::finish(http_request_body, None)
            .expect("failed to finish sending request body");

        let http_response = http::outgoing_handler::handle(http_request, None)
            .expect("failed to handle HTTP request");
        let http_response_sub = http_response.subscribe();

        // No args, return string
        let pong = pingpong::ping();
        // Number arg, return number
        let meaning_of_universe = busybox::increment_number(41);
        // Multiple args, return vector of strings
        let other: Vec<String> = busybox::string_split("hi,there,friend", ',');
        // Variant / Enum argument, return bool
        let is_same = busybox::string_assert(busybox::Easyasonetwothree::A, "a");
        let doggo = busybox::Dog {
            name: "Archie".to_string(),
            age: 3,
        };
        // Record / struct argument
        let is_good_boy = busybox::is_good_boy(&doggo);

        assert_eq!(poll(&[&http_response_sub]), [0]);
        let http_response = http_response
            .get()
            .expect("HTTP request response missing")
            .expect("HTTP request response requested more than once")
            .expect("HTTP request failed");
        assert_eq!(http_response.status(), 200);

        // TODO: Assert headers
        _ = http_response.headers();
        let http_response_body = http_response
            .consume()
            .expect("failed to get incoming request body");
        {
            let mut buf = vec![];
            let mut stream = http_response_body
                .stream()
                .expect("failed to get HTTP request response stream");
            InputStreamReader::from(&mut stream)
                .read_to_end(&mut buf)
                .expect("failed to read value from HTTP request response stream");
            assert_eq!(buf, b"test");
        };
        // TODO: Assert trailers
        let _trailers = http::types::IncomingBody::finish(http_response_body);

        format!("Ping {pong}, meaning of universe is: {meaning_of_universe}, split: {other:?}, is_same: {is_same}, archie good boy: {is_good_boy}")
    }
}
