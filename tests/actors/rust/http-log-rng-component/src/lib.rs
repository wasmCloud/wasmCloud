wit_bindgen::generate!("actor");

use serde::Deserialize;
use serde_json::json;
use wasmcloud_actor::wasi::logging::logging;
use wasmcloud_actor::wasi::random::random;
use wasmcloud_actor::{debug, error, info, trace, warn, HostRng};
use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse};

struct Actor;

impl exports::wasmcloud::bus::guest::Guest for Actor {
    fn call(operation: String, payload: Option<Vec<u8>>) -> Result<Option<Vec<u8>>, String> {
        assert_eq!(operation, "HttpServer.HandleRequest");
        let payload = payload.expect("missing payload");
        let HttpRequest {
            method,
            path,
            query_string,
            header,
            body,
        } = rmp_serde::from_slice(payload.as_ref()).expect("failed to deserialize request");
        assert!(method.is_empty());
        assert!(path.is_empty());
        assert!(query_string.is_empty());
        assert!(header.is_empty());

        #[derive(Deserialize)]
        struct Request {
            min: u32,
            max: u32,
        }
        let Request { min, max } =
            serde_json::from_slice(&body).expect("failed to decode request body");

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

        let res = json!({
            "get_random_bytes": random::get_random_bytes(8),
            "get_random_u64": random::get_random_u64(),
            "guid": HostRng::generate_guid(),
            "random_32": HostRng::random32(),
            "random_in_range": HostRng::random_in_range(min, max),
        });
        eprintln!("response: `{res:?}`");
        let body = serde_json::to_vec(&res).expect("failed to encode response to JSON");
        let res = rmp_serde::to_vec(&HttpResponse {
            body,
            ..Default::default()
        })
        .expect("failed to serialize response");
        Ok(Some(res))
    }
}

export_actor!(Actor);
