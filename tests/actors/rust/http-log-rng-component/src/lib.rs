#![cfg(target_arch = "wasm32")]

use serde::Deserialize;
use serde_json::json;
use wasmbus_rpc::common::{deserialize, serialize};
use wasmcloud_interface_httpserver::{HttpRequest, HttpResponse};
use wasmcloud_interface_logging::LogEntry;
use wasmcloud_interface_numbergen::RangeLimit;

wit_bindgen::generate!({
    world: "wasmcloud",
    path: "../../../../wit",
});

struct Actor;

fn log(level: impl Into<String>, text: impl Into<String>) {
    let entry = serialize(&LogEntry {
        level: level.into(),
        text: text.into(),
    })
    .expect("failed to serialize log entry");
    host::host_call(
        "default",
        "wasmcloud:builtin:logging",
        "Logging.WriteLog",
        Some(&entry),
    )
    .expect("failed to log entry");
}

impl actor::Actor for Actor {
    fn guest_call(operation: String, payload: Option<Vec<u8>>) -> Result<Option<Vec<u8>>, String> {
        assert_eq!(operation, "HttpServer.HandleRequest");
        let payload = payload.expect("missing payload");
        let HttpRequest {
            method,
            path,
            query_string,
            header,
            body,
        } = deserialize(payload.as_ref()).expect("failed to deserialize request");
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

        log("debug", "debug");
        log("info", "info");
        log("warn", "warn");
        log("error", "error");

        let guid = host::host_call(
            "default",
            "wasmcloud:builtin:numbergen",
            "NumberGen.GenerateGuid",
            None,
        )
        .expect("failed to generate GUID")
        .expect("missing GUID");
        let guid: String = deserialize(&guid).expect("failed to deserialize GUID");

        let limit = serialize(&RangeLimit { min, max }).expect("failed to serialize range");
        let r_range = host::host_call(
            "default",
            "wasmcloud:builtin:numbergen",
            "NumberGen.RandomInRange",
            Some(&limit),
        )
        .expect("failed to generate random u32 in range")
        .expect("missing random u32 in range");
        let r_range: u32 =
            deserialize(&r_range).expect("failed to deserialize random u32 in range");

        let r_32 = host::host_call(
            "default",
            "wasmcloud:builtin:numbergen",
            "NumberGen.Random32",
            None,
        )
        .expect("failed to generate random u32")
        .expect("missing random u32");
        let r_32: u32 = deserialize(&r_32).expect("failed to deserialize random u32");

        let body = serde_json::to_vec(&json!({
            "guid": guid,
            "random_in_range": r_range,
            "random_32": r_32,
        }))
        .expect("failed to encode response to JSON");

        let res = serialize(&HttpResponse {
            body,
            ..Default::default()
        })
        .expect("failed to serialize response");

        Ok(Some(res))
    }
}

export_wasmcloud!(Actor);
