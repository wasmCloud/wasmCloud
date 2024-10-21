use wasi::http::types::*;
use wasmcloud_component::http::{Request, Response};

#[derive(serde::Deserialize)]
struct DogResponse {
    message: String,
}

struct DogFetcher;

wasmcloud_component::http::export!(DogFetcher);

impl wasmcloud_component::http::HttpServer for DogFetcher {
    fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let http_response: reqwest::Response =
            reqwest::get("https://dog.ceo/api/breeds/image/random").map_err(|e| {
                ErrorCode::InternalError(Some(format!("Failed to fetch dog: {}", e)))
            })?;
        let dog_response: DogResponse = http_response
            .bytes()
            .map(|response_bytes| {
                serde_json::from_slice(&response_bytes).map_err(|e| {
                    ErrorCode::InternalError(Some(format!("Failed to parse dog response: {}", e)))
                })
            })
            .map_err(|e| {
                ErrorCode::InternalError(Some(format!("Failed to read dog response: {}", e)))
            })??;

        Ok(Response::ok(dog_response.message.into()))
    }
}
