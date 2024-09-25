# wasmCloud Component

wasmCloud is an open source Cloud Native Computing Foundation (CNCF) project that enables teams to build, manage, and scale polyglot Wasm apps across any cloud, K8s, or edge.

⚠️ This crate is highly experimental and likely to experience breaking changes frequently. The host itself is relatively stable, but the APIs and public members of this crate are not guaranteed to be stable and may change in a backwards-incompatible way.

## Usage

This crate is a collection of `wasi` and `wasmcloud` interfaces that can be used at runtime in a Wasm component running in wasmCloud. It can be imported in a Rust application and used directly rather than generating bindings manually in code.

```rust
wit_bindgen::generate!({
    with: {
        "wasi:http/types@0.2.1": wasmcloud_component::wasi::http::types,
        "wasi:io/streams@0.2.1": wasmcloud_component::wasi::io::streams,
    }
});

use std::collections::HashMap;
use std::io::{Read, Write};

use serde::Deserialize;
use serde_json::json;
use test_components::testing::*;
use wasmcloud_component::wasi::{config, http};
use wasmcloud_component::{InputStreamReader, OutputStreamWriter};

struct Component;

impl exports::wasi::http::incoming_handler::Guest for Component {
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
        let pong_secret = pingpong::ping_secret();

        let res = json!({
            "single_val": config::runtime::get(&config_key).expect("failed to get config value"),
            "multi_val": config::runtime::get_all().expect("failed to get config value").into_iter().collect::<HashMap<String, String>>(),
            "pong": pong,
            "pong_secret": pong_secret,
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

export!(Component);
```
