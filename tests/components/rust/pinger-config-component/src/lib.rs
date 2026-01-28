wit_bindgen::generate!({
    with: {
        "wasi:http/types@0.2.9": wasmcloud_component::wasi::http::types,
        "wasi:io/streams@0.2.9": wasmcloud_component::wasi::io::streams,
    },
    generate_all,
});

use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use test_components::testing::*;
use wasmcloud_component::http;
use wasmcloud_component::wasi::config;

struct Actor;

impl http::Server for Actor {
    fn handle(
        request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        #[derive(Deserialize)]
        struct Request {
            config_key: String,
        }

        let mut body = request.into_body();
        let Request { config_key } =
            serde_json::from_reader(&mut body).expect("failed to decode request body");
        let trailers = body.into_trailers().expect("failed to get trailers");
        assert!(trailers.is_none());

        // No args, return string
        let pong = pingpong::ping();
        let pong_secret = pingpong::ping_secret();

        let res = json!({
            "single_val": config::store::get(&config_key).expect("failed to get config value"),
            "multi_val": config::store::get_all().expect("failed to get config value").into_iter().collect::<HashMap<String, String>>(),
            "pong": pong,
            "pong_secret": pong_secret,
        });
        let body = serde_json::to_vec(&res).expect("failed to encode response to JSON");
        Ok(http::Response::new(body))
    }
}

http::export!(Actor);
