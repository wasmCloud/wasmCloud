wit_bindgen::generate!({
    with: {
        "wasi:http/types@0.2.9": wasmcloud_component::wasi::http::types,
        "wasi:io/streams@0.2.9": wasmcloud_component::wasi::io::streams,
        "wasmcloud:identity/store@0.0.1": generate,
    },
    generate_all,
});

use serde_json::json;
use wasmcloud::identity::*;
use wasmcloud_component::http;

struct WorkloadIdentityTest;

http::export!(WorkloadIdentityTest);

impl http::Server for WorkloadIdentityTest {
    fn handle(
        _request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        let test_audience = "spiffe://wasmcloud.dev/workload-identity-test";

        let response_json = match store::get(test_audience) {
            Ok(token) => {
                json!({
                    "token": token.unwrap_or_default()
                })
            }
            Err(err) => {
                json!({
                    "error": err.to_string()
                })
            }
        };

        let body = serde_json::to_vec(&response_json).expect("failed to encode response to JSON");
        Ok(http::Response::new(body))
    }
}
