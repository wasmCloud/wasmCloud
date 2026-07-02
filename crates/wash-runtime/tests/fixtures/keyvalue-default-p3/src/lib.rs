//! Real-guest fixture for a PLAIN (unlabeled) async `wasmcloud:keyvalue` import.
//!
//! Unlike `keyvalue-implements-p3`, this opens a bucket through
//! `wasmcloud:keyvalue/store` *without* an `(implements ..)` label. On each HTTP
//! request it opens a bucket, sets a key, increments a counter through the
//! standalone `atomics` import, reads the value back, and returns it — proving
//! the host binds a default backend for a plain `store` import (no label needed).

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::keyvalue::{atomics, store};

struct Component;

const BUCKET: &str = "default-kv";
const KEY: &str = "greeting";
const COUNTER: &str = "hits";
const BODY: &[u8] = b"woof from a plain p3 keyvalue guest";

fn internal(msg: String) -> ErrorCode {
    ErrorCode::InternalError(Some(msg))
}

fn respond(status: u16, body_bytes: Vec<u8>) -> Result<Response, ErrorCode> {
    let headers = Fields::new();
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());

    wit_bindgen::spawn_local(async move {
        tx.write_all(body_bytes).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });

    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    response
        .set_status_code(status)
        .map_err(|()| internal("failed to set status".into()))?;
    Ok(response)
}

async fn run() -> Result<Vec<u8>, String> {
    // Open a bucket through the PLAIN `store` import — no label, so the host must
    // route it to a default backend.
    let bucket = store::open(BUCKET.to_string())
        .await
        .map_err(|e| format!("open: {e:?}"))?;

    bucket
        .set(KEY.to_string(), BODY.to_vec(), None)
        .await
        .map_err(|e| format!("set: {e:?}"))?;

    // The standalone `atomics` import operates on the plainly-opened bucket,
    // exercising the resource-routed path alongside the default `store` binding.
    atomics::increment(&bucket, COUNTER.to_string(), 1)
        .await
        .map_err(|e| format!("increment: {e:?}"))?;

    let value = bucket
        .get(KEY.to_string())
        .await
        .map_err(|e| format!("get: {e:?}"))?
        .ok_or_else(|| "key missing after set".to_string())?;
    Ok(value)
}

impl Handler for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        match run().await {
            Ok(value) => respond(200, value),
            Err(e) => respond(500, format!("error: {e}").into_bytes()),
        }
    }
}

bindings::export!(Component with_types_in bindings);
