use serde_json::json;
use wasmcloud_actor::{export_actor, HttpHandler, HttpRequest, HttpResponse};

#[derive(Default)]
struct Echo;

impl HttpHandler for Echo {
    fn handle_request(
        &self,
        HttpRequest {
            method,
            path,
            query_string,
            body,
            header,
        }: HttpRequest,
    ) -> Result<HttpResponse, String> {
        let body = serde_json::to_vec(&json!({
            "method": method,
            "path": path,
            "query_string": query_string,
            "body": body,
            "header": header,
        }))
        .map_err(|e| format!("failed to serialize response: {e}"))?;
        Ok(HttpResponse {
            body,
            ..Default::default()
        })
    }
}

export_actor!(Echo, HttpHandler);
