wit_bindgen::generate!({
    world: "component",
    with: {
        "test-components:testing/pingpong@0.1.0": generate,
        "wasmcloud:bus/lattice@2.0.0": generate,
        "wasi:http/types@0.2.2": wasmcloud_component::wasi::http::types,
        "wasi:io/streams@0.2.2": wasmcloud_component::wasi::io::streams,
    },
    generate_all,
});

use test_components::testing::*;
use wasmcloud_component::http;

struct Component;

impl http::Server for Component {
    fn handle(
        _request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        let iface = wasmcloud::bus::lattice::CallTargetInterface::new(
            "test-components",
            "testing",
            "pingpong",
        );
        wasmcloud::bus::lattice::set_target_component("mock-server", iface, "0.1.0")
            .map_err(|e| http::ErrorCode::InternalError(Some(e)))?;
        let res = pingpong::ping();
        let body = if res == "Pong from external!" {
            "External ping successful!".to_string()
        } else {
            format!("External ping failed: Unexpected response: {}", res)
        };
        Ok(http::Response::new(body.into_bytes()))
    }
}

http::export!(Component);
