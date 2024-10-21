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

// Error example
// impl HttpServer for Component {
//     fn handle(_request: Request) -> Result<ResponseBuilder, ErrorCode> {
//         Err(ErrorCode::InternalError(Some(
//             "Oopsie woopsie! We made an ewwow.".to_string(),
//         )))
//     }
// }

// Streaming example
// impl HttpServer for Component {
//     fn handle(request: Request) -> Result<ResponseBuilder, ErrorCode> {
//         let (stream, body) = request.into_body_stream()?;
//         Ok(ResponseBuilder::stream_body(stream, Some(body)).status_code(200))
//     }
// }

// Outgoing request example
// impl HttpServer for Component {
//     fn handle(_request: Request) -> Result<ResponseBuilder, ErrorCode> {
//         let response: reqwest::Response = reqwest::get("https://example.com").map_err(|e| {
//             ErrorCode::InternalError(Some(format!("failed to send outbound request {e:?}")))
//         })?;
//         let example_dot_com = response.bytes().map_err(|e| {
//             ErrorCode::InternalError(Some(format!("failed to read response body {e:?}")))
//         })?;
//         Ok(ResponseBuilder::ok(example_dot_com))
//     }
// }

// Outgoing request streaming body back example
// impl HttpServer for Component {
//     fn handle(_request: Request) -> Result<ResponseBuilder, ErrorCode> {
//         let mut response: reqwest::Response = reqwest::get("https://example.com").map_err(|e| {
//             ErrorCode::InternalError(Some(format!("failed to send outbound request {e:?}")))
//         })?;
//         let (stream, body) = response.bytes_stream().map_err(|e| {
//             ErrorCode::InternalError(Some(format!("failed to read response body {e:?}")))
//         })?;
//         Ok(ResponseBuilder::stream_body(stream, Some(body)).status_code(200))
//     }
// }
