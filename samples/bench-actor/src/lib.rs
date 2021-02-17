extern crate wapc_guest as guest;
use actor::deserialize;
use serde::{Deserialize, Serialize};
use wasmcloud_actor_core as actor;
use wasmcloud_actor_http_server as http;

use guest::prelude::*;

#[no_mangle]
pub fn wapc_init() {
    actor::Handlers::register_health_request(health);
    http::Handlers::register_handle_request(handle_http);
}

fn handle_http(req: http::Request) -> HandlerResult<http::Response> {
    let mut data: Data = deserialize(&req.body)?;

    data.field1 = data.field1.sin();
    data.field2 = 2 * (data.field2 + 150);

    Ok(http::Response::json(&data, 200, "OK"))
}

fn health(_h: actor::HealthCheckRequest) -> HandlerResult<actor::HealthCheckResponse> {
    Ok(actor::HealthCheckResponse::healthy())
}

// Copy this struct into whatever benchmark test is invoking this actor
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Data {
    pub field1: f64,
    pub field2: u32,
    pub field3: String,
}
