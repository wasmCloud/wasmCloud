#![cfg(target_arch = "wasm32")]

use serde::Deserialize;
use serde_json::json;
use wasmbus_rpc::actor::prelude::*;
use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse, HttpServer, HttpServerReceiver};
use wasmcloud_interface_logging::{debug, error, info, warn};
use wasmcloud_interface_numbergen::{generate_guid, random_32, random_in_range};

#[derive(Debug, Default, Actor, HealthResponder)]
#[services(Actor, HttpServer)]
struct HttpLogRng;

#[async_trait]
impl HttpServer for HttpLogRng {
    async fn handle_request(
        &self,
        _: &Context,
        HttpRequest { body, .. }: &HttpRequest,
    ) -> RpcResult<HttpResponse> {
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
            serde_json::from_slice(body).expect("failed to decode request body");

        let guid = generate_guid()
            .await
            .expect("failed to call `generate_guid`");
        let r_range = random_in_range(min, max)
            .await
            .expect("failed to call `random_in_range`");
        let r_32 = random_32().await.expect("failed to call `random_32`");

        let body = serde_json::to_vec(&json!({
            "guid": guid,
            "random_in_range": r_range,
            "random_32": r_32,
        }))
        .expect("failed to encode response to JSON");
        Ok(HttpResponse {
            body,
            ..Default::default()
        })
    }
}
