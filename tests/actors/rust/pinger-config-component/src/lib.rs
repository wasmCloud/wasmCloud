#![allow(clippy::missing_safety_doc)]

wit_bindgen::generate!({
    world: "actor",
    with: {
        "wasi:io/streams@0.2.0": wasmcloud_actor::wasi::io::streams,
    }
});

use std::{
    collections::HashMap,
    io::{Read, Write},
};

use serde::Deserialize;
use serde_json::json;
use test_actors::testing::*;
use wasi::http;
use wasmcloud_actor::{InputStreamReader, OutputStreamWriter};

struct Actor;

impl exports::wasi::http::incoming_handler::Guest for Actor {
    fn handle(request: http::types::IncomingRequest, response_out: http::types::ResponseOutparam) {
        #[derive(Deserialize)]
        struct Request {
            config_key: String,
        }

        let request_body = request
            .consume()
            .expect("failed to get incoming request body");
        let Request { config_key } = {
            let mut buf = vec![];
            let mut stream = request_body
                .stream()
                .expect("failed to get incoming request stream");
            InputStreamReader::from(&mut stream)
                .read_to_end(&mut buf)
                .expect("failed to read value from incoming request stream");
            serde_json::from_slice(&buf).expect("failed to decode request body")
        };
        let _trailers = http::types::IncomingBody::finish(request_body);

        // No args, return string
        let pong = pingpong::ping();

        let res = json!({
            "single_val": wasmcloud::bus::guest_config::get(&config_key).expect("failed to get config value").map(|s| String::from_utf8(s).expect("Config value should be a string")),
            "multi_val": wasmcloud::bus::guest_config::get_all().expect("failed to get config value").into_iter().map(|(k, v)| (k, String::from_utf8(v).expect("Config value should be a string"))).collect::<HashMap<String, String>>(),
            "pong": pong,
        });
        let body = serde_json::to_vec(&res).expect("failed to encode response to JSON");
        let response = http::types::OutgoingResponse::new(http::types::Fields::new());
        let response_body = response
            .body()
            .expect("failed to get outgoing response body");
        {
            let mut stream = response_body
                .write()
                .expect("failed to get outgoing response stream");
            let mut w = OutputStreamWriter::from(&mut stream);
            w.write_all(&body)
                .expect("failed to write body to outgoing response stream");
            w.flush().expect("failed to flush outgoing response stream");
        }
        http::types::OutgoingBody::finish(response_body, None)
            .expect("failed to finish response body");
        http::types::ResponseOutparam::set(response_out, Ok(response));
    }
}

export!(Actor);
