use wasmcloud_component::http;
use std::{thread::sleep, time::Duration};

struct Component;

http::export!(Component);

impl http::Server for Component {
    fn handle(
        _request: http::IncomingRequest,
    ) -> http::Result<http::Response<impl http::OutgoingBody>> {
        sleep(Duration::from_secs(3));
        Ok(http::Response::new("Hello from Rust!\n"))
    }
}
