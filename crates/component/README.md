# wasmCloud Component

wasmCloud is an open source Cloud Native Computing Foundation (CNCF) project that enables teams to build, manage, and scale polyglot Wasm apps across any cloud, K8s, or edge. This crate provides pre-generated interfaces and idiomatic wrappers that aid in building [WebAssembly components](https://component-model.bytecodealliance.org/introduction.html) via the `wasm32-wasip2` target.

⚠️ This crate is highly experimental and likely to experience breaking changes frequently. The host itself is relatively stable, but the APIs and public members of this crate are not guaranteed to be stable and may change in a backwards-incompatible way.

## Usage

This crate is a collection of `wasi` and `wasmcloud` interfaces that can be used at runtime in a Wasm component running in wasmCloud. It can be imported in a Rust application and used directly rather than generating bindings manually in code.

```rust
// This crate can be used with `wit_bindgen` for interoperability with WIT types
// wit_bindgen::generate!({
//     with: {
//         "wasi:http/types@0.2.1": wasmcloud_component::wasi::http::types,
//         "wasi:io/streams@0.2.1": wasmcloud_component::wasi::io::streams,
//     },
//     generate_all
// });

use std::io::Read;

use serde::Deserialize;
use serde_json::json;
use wasmcloud_component::debug;
use wasmcloud_component::http::{self, ErrorCode};
use wasmcloud_component::wasi::{config, keyvalue};

struct Component;

// The `http::Server` trait is a wrapper around `wasi:http/incoming-handler` that implements
// the `handle` function with the standard `http` crate.
impl http::Server for Component {
    fn handle(
        request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        // Use macros for leveled `wasi:logging` logging
        debug!("handling request");

        let (parts, mut body) = request.into_parts();

        #[derive(Deserialize)]
        struct UserRequest {
            config_key: String,
        }

        // Use `http` and `serde_json` like a normal Rust HTTP server
        let UserRequest { config_key } = {
            let mut buf = vec![];
            body.read_to_end(&mut buf).map_err(|_| {
                ErrorCode::InternalError(Some("failed to read request body".to_string()))
            })?;
            serde_json::from_slice(&buf).map_err(|_| {
                ErrorCode::InternalError(Some("failed to decode request body".to_string()))
            })?
        };

        // Use `wit_bindgen` generated interfaces to call other components or
        // capability providers.
        let pong = pingpong::ping();

        // Use the `wasi` crate for wasi-cloud interfaces
        // like `keyvalue` and `blobstore`
        let kv_key = parts.uri.path();
        let cache = keyvalue::store::open("default").expect("open bucket");
        let count = keyvalue::atomics::increment(&cache, kv_key, 1).map_err(|_| {
            ErrorCode::InternalError(Some("failed to increment counter in store".to_string()))
        })?;

        // Use `wasi:config/store` to configure at runtime
        let single_val = config::store::get(&config_key).map_err(|_| {
            ErrorCode::InternalError(Some("failed to get config value".to_string()))
        })?;
        let multi_val = config::store::get_all().map_err(|_| {
            ErrorCode::InternalError(Some("failed to get config value".to_string()))
        })?;

        let res = json!({
            "single_val": single_val,
            "multi_val": multi_val,
            "count": count,
            "pong": pong,
        });

        // Encode and send response
        let body = serde_json::to_vec(&res).expect("failed to encode response to JSON");
        Ok(http::Response::new(body))
    }
}

http::export!(Component);
```
