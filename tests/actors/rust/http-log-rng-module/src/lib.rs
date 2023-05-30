use serde::Deserialize;
use serde_json::json;
use wasmcloud_actor::{
    debug, error, export_actor, info, warn, HostRng, HttpHandler, HttpRequest, HttpResponse,
};

#[derive(Default)]
struct HttpLogRng;

impl HttpHandler for HttpLogRng {
    fn handle_request(
        &self,
        HttpRequest { body, .. }: HttpRequest,
    ) -> Result<HttpResponse, String> {
        debug!("debug");
        info!("info");
        warn!("warn");
        error!("error");

        #[derive(Deserialize)]
        struct Request {
            min: u32,
            max: u32,
        }
        let Request { min, max } =
            serde_json::from_slice(&body).expect("failed to decode request body");

        let body = serde_json::to_vec(&json!({
            "guid": HostRng::generate_guid(),
            "random_in_range": HostRng::random_in_range(min, max),
            "random_32": HostRng::random32(),
        }))
        .expect("failed to encode response to JSON");
        Ok(HttpResponse {
            body,
            ..Default::default()
        })
    }
}

export_actor!(HttpLogRng, HttpHandler);
