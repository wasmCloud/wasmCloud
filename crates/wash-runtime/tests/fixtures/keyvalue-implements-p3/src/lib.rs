//! Real-guest ABA test for async `wasmcloud:keyvalue` compare-and-swap.
//!
//! Opens a bucket through the `(implements ..)`-labeled `kv` (store) import, then
//! drives an A → B → A sequence and a version-pinned `cas.swap` through the
//! standalone `cas` import. Because the value returns to identical bytes, a
//! content-hash version would let the swap wrongly succeed; a backend-native
//! monotonic version makes it `stale`. The handler answers `aba-detected` only
//! when the swap is correctly rejected.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::keyvalue::cas::{self, CasOptions, CasResult};
use bindings::wasmcloud::keyvalue::{atomics, batch};

struct Component;

const BUCKET: &str = "aba";
const KEY: &str = "k";

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

async fn run() -> Result<String, String> {
    // `kv` is the labeled `store` import; `cas` is standalone but operates on the
    // same `bucket` resource.
    let bucket = bindings::kv::open(BUCKET.to_string())
        .await
        .map_err(|e| format!("open: {e:?}"))?;

    bucket
        .set(KEY.to_string(), b"A".to_vec(), None)
        .await
        .map_err(|e| format!("set A: {e:?}"))?;

    let v1 = cas::current(&bucket, KEY.to_string())
        .await
        .map_err(|e| format!("current: {e:?}"))?
        .ok_or_else(|| "key missing after set".to_string())?
        .version;

    // A -> B -> A: the value returns to identical bytes.
    bucket
        .set(KEY.to_string(), b"B".to_vec(), None)
        .await
        .map_err(|e| format!("set B: {e:?}"))?;
    bucket
        .set(KEY.to_string(), b"A".to_vec(), None)
        .await
        .map_err(|e| format!("set A again: {e:?}"))?;

    // (1) A swap pinned to the original version (v1) must be reported stale.
    // The key changed A -> B -> A, so its bytes are back to the original. With a
    // monotonic version the version still moved, so the swap is correctly stale;
    // a content-hash version would match again and let the swap through.
    let aba = cas::swap(
        &bucket,
        KEY.to_string(),
        b"C".to_vec(),
        CasOptions {
            require_version: Some(v1),
            require_value: None,
        },
    )
    .await
    .map_err(|e| format!("aba swap: {e:?}"))?;
    if !matches!(aba, CasResult::Stale(_)) {
        return Ok("fail: A->B->A swap on the old version should be stale".to_string());
    }

    // (2) Swap pinned to the CURRENT version must succeed — proves CAS actually
    // works, so the stale result above isn't just an always-stale backend.
    let current_version = cas::current(&bucket, KEY.to_string())
        .await
        .map_err(|e| format!("current after aba: {e:?}"))?
        .ok_or_else(|| "key vanished".to_string())?
        .version;
    let positive = cas::swap(
        &bucket,
        KEY.to_string(),
        b"D".to_vec(),
        CasOptions {
            require_version: Some(current_version),
            require_value: None,
        },
    )
    .await
    .map_err(|e| format!("positive swap: {e:?}"))?;
    if !matches!(positive, CasResult::Swapped) {
        return Ok("fail: swap on the current version should succeed".to_string());
    }

    // (3) Empty cas-options (no precondition) must be rejected with
    // invalid-argument, not silently performed as an unconditional write.
    let empty = cas::swap(
        &bucket,
        KEY.to_string(),
        b"E".to_vec(),
        CasOptions {
            require_version: None,
            require_value: None,
        },
    )
    .await;
    match empty {
        Err(e) if format!("{e:?}").contains("InvalidArgument") => {}
        other => return Ok(format!("fail: empty cas-options not rejected: {other:?}")),
    }

    // (4) atomics.increment through the standalone `atomics` import, on the same
    // labeled-store bucket. Absent counter starts at 0: +5 -> 5, +3 -> 8.
    let c1 = atomics::increment(&bucket, "counter".to_string(), 5)
        .await
        .map_err(|e| format!("increment 1: {e:?}"))?;
    let c2 = atomics::increment(&bucket, "counter".to_string(), 3)
        .await
        .map_err(|e| format!("increment 2: {e:?}"))?;
    if (c1, c2) != (5, 8) {
        return Ok(format!("fail: increment expected (5,8), got ({c1},{c2})"));
    }

    // (5) batch set-many / get-many / delete-many through the standalone `batch`
    // import. get-many must return values positionally, with `none` for a miss.
    batch::set_many(
        &bucket,
        vec![
            ("b1".to_string(), b"v1".to_vec()),
            ("b2".to_string(), b"v2".to_vec()),
        ],
    )
    .await
    .map_err(|e| format!("set_many: {e:?}"))?;
    let got = batch::get_many(
        &bucket,
        vec!["b1".to_string(), "b2".to_string(), "missing".to_string()],
    )
    .await
    .map_err(|e| format!("get_many: {e:?}"))?;
    let values: Vec<Option<Vec<u8>>> = got.into_iter().map(|kv| kv.map(|(_, v)| v)).collect();
    if values != vec![Some(b"v1".to_vec()), Some(b"v2".to_vec()), None] {
        return Ok(format!("fail: get_many mismatch: {values:?}"));
    }
    batch::delete_many(&bucket, vec!["b1".to_string()])
        .await
        .map_err(|e| format!("delete_many: {e:?}"))?;
    let after = batch::get_many(&bucket, vec!["b1".to_string(), "b2".to_string()])
        .await
        .map_err(|e| format!("get_many after delete: {e:?}"))?;
    if after[0].is_some() || after[1].is_none() {
        return Ok(format!("fail: delete_many left {after:?}"));
    }

    Ok("ok".to_string())
}

impl Handler for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let body = match run().await {
            Ok(s) => s,
            Err(e) => format!("error: {e}"),
        };
        respond(200, body.into_bytes())
    }
}

bindings::export!(Component with_types_in bindings);
