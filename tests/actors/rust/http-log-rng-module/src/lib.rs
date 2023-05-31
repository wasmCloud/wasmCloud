use serde::Deserialize;
use serde_json::json;
use wasmcloud_actor::{
    debug, error, export_actor, info, logging, random, trace, warn, HostRng, HttpHandler,
    HttpRequest, HttpResponse,
};

#[derive(Default)]
struct HttpLogRng;

impl HttpHandler for HttpLogRng {
    fn handle_request(
        &self,
        HttpRequest { body, .. }: HttpRequest,
    ) -> Result<HttpResponse, String> {
        logging::log(logging::Level::Trace, "trace-context", "trace");
        logging::log(logging::Level::Debug, "debug-context", "debug");
        logging::log(logging::Level::Info, "info-context", "info");
        logging::log(logging::Level::Warn, "warn-context", "warn");
        logging::log(logging::Level::Error, "error-context", "error");

        trace!(context: "trace-context", "trace");
        debug!(context: "debug-context", "debug");
        info!(context: "info-context", "info");
        warn!(context: "warn-context", "warn");
        error!(context: "error-context", "error");

        trace!("trace");
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

        let res = json!({
            "get_random_bytes": random::get_random_bytes(8),
            "get_random_u64": random::get_random_u64(),
            "guid": HostRng::generate_guid(),
            "random_32": HostRng::random32(),
            "random_in_range": HostRng::random_in_range(min, max),
        });
        eprintln!("response: `{res:?}`");
        let body = serde_json::to_vec(&res).expect("failed to encode response to JSON");
        Ok(HttpResponse {
            body,
            ..Default::default()
        })
    }
}

export_actor!(HttpLogRng, HttpHandler);
