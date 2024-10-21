use wasi::http::types::ErrorCode;
use wasmcloud_component::http::{HttpServer, Request, Response};

struct Component;

wasmcloud_component::http::export!(Component);

// Implementing the [`HttpServer`] trait for a component
impl HttpServer for Component {
    fn handle(_request: Request) -> Result<Response, ErrorCode> {
        Ok(Response::ok("Hello from Rust!".into()))
    }
}
