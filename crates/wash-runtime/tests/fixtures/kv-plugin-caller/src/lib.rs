//! Workload component driving the imported `acme:kv/store` capability over HTTP.
//! The host resolves that import to a host component plugin running in its own
//! store, so every operation below is a cross-store call the test can drive by
//! hitting an HTTP endpoint:
//!
//! - `GET /set?key=K&value=V` -> `store.set(K, V)`         -> `{"ok":true}`
//! - `GET /get?key=K`         -> `store.get(K)`            -> 200 body=V, or 404
//! - `GET /delete?key=K`      -> `store.delete(K)`         -> `{"ok":true}`
//! - `GET /slow?millis=N`     -> `store.slow(N)`           -> 200 body=N
//!
//! Values are treated as UTF-8 strings for easy assertions; tests keep them
//! ASCII so no URL decoding is needed.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "caller", generate_all });
}

use bindings::acme::kv::store;
use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

struct Component;

impl HttpGuest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request
            .get_path_with_query()
            .unwrap_or_else(|| "/".to_string());
        let (route, query) = match path.split_once('?') {
            Some((r, q)) => (r, q),
            None => (path.as_str(), ""),
        };

        if route.starts_with("/set") {
            let key = query_get(query, "key").unwrap_or_default();
            let value = query_get(query, "value").unwrap_or_default();
            store::set(key, value.into_bytes()).await;
            return Ok(make_response(200, b"{\"ok\":true}".to_vec()));
        }
        if route.starts_with("/pset") {
            // Per-caller partitioned set: isolated to THIS workload.
            let key = query_get(query, "key").unwrap_or_default();
            let value = query_get(query, "value").unwrap_or_default();
            store::pset(key, value.into_bytes()).await;
            return Ok(make_response(200, b"{\"ok\":true}".to_vec()));
        }
        if route.starts_with("/pget") {
            let key = query_get(query, "key").unwrap_or_default();
            return match store::pget(key).await {
                Some(v) => Ok(make_response(200, v)),
                None => Ok(make_response(404, Vec::new())),
            };
        }
        if route.starts_with("/get") {
            let key = query_get(query, "key").unwrap_or_default();
            return match store::get(key).await {
                Some(v) => Ok(make_response(200, v)),
                None => Ok(make_response(404, Vec::new())),
            };
        }
        if route.starts_with("/delete") {
            let key = query_get(query, "key").unwrap_or_default();
            store::delete(key).await;
            return Ok(make_response(200, b"{\"ok\":true}".to_vec()));
        }
        if route.starts_with("/slow") {
            let millis: u64 = query_get(query, "millis")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let echoed = store::slow(millis).await;
            return Ok(make_response(200, echoed.to_string().into_bytes()));
        }
        if route.starts_with("/total") {
            // caller -> plugin stream: send `count` bytes, plugin totals them.
            let count: u64 = query_get(query, "count")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let (mut tx, rx) = bindings::wit_stream::new();
            wit_bindgen::spawn_local(async move {
                let chunk = vec![b'q'; 256];
                let mut written: u64 = 0;
                while written < count {
                    let n = ((count - written) as usize).min(chunk.len());
                    tx.write_all(chunk[..n].to_vec()).await;
                    written += n as u64;
                }
                drop(tx);
            });
            let total = store::total(rx).await;
            return Ok(make_response(200, total.to_string().into_bytes()));
        }
        if route.starts_with("/emit") {
            // plugin -> caller stream: drain the plugin's stream, count bytes.
            let count: u64 = query_get(query, "count")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let mut rx = store::emit(count).await;
            let mut total: u64 = 0;
            loop {
                let (result, chunk) = rx.read(Vec::with_capacity(4096)).await;
                total += chunk.len() as u64;
                if matches!(result, wit_bindgen::StreamResult::Dropped) {
                    break;
                }
            }
            return Ok(make_response(200, total.to_string().into_bytes()));
        }
        if route.starts_with("/eventually") {
            // plugin -> caller future: await the resolved value.
            let value: u64 = query_get(query, "value")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let reader = store::eventually(value).await;
            let resolved = reader.await;
            return Ok(make_response(200, resolved.to_string().into_bytes()));
        }
        if route.starts_with("/recurse") {
            // Drives the plugin's re-entrant self-recursion; a large `n` trips
            // the host depth guard and traps (surfaced as a 500).
            let n: u64 = query_get(query, "n")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let depth = store::recurse(n).await;
            return Ok(make_response(200, depth.to_string().into_bytes()));
        }
        if route.starts_with("/boom") {
            // Triggers a guest trap in the plugin; the caller's request fails.
            store::boom().await;
            return Ok(make_response(200, b"unreachable".to_vec()));
        }
        if route.starts_with("/bucket-get") {
            // Open a FRESH bucket and read a key WITHOUT setting it — 404 unless
            // buckets leak state into each other (they must not: each `open` is
            // its own resource, so a fresh bucket never sees another's keys).
            let name = query_get(query, "name").unwrap_or_default();
            let key = query_get(query, "key").unwrap_or_default();
            let bucket = store::open(name).await;
            let got = bucket.get(key).await;
            drop(bucket);
            return match got {
                Some(v) => Ok(make_response(200, v)),
                None => Ok(make_response(404, Vec::new())),
            };
        }
        if route.starts_with("/bucket") {
            // Cross-store resource: open a bucket (own<bucket> proxy), set then
            // get a key through it (borrow<bucket> methods), and drop it so the
            // real resource in the plugin store is freed.
            let name = query_get(query, "name").unwrap_or_default();
            let key = query_get(query, "key").unwrap_or_default();
            let value = query_get(query, "value").unwrap_or_default();
            let bucket = store::open(name).await;
            bucket.set(key.clone(), value.into_bytes()).await;
            let got = bucket.get(key).await;
            drop(bucket);
            return match got {
                Some(v) => Ok(make_response(200, v)),
                None => Ok(make_response(404, Vec::new())),
            };
        }
        if route.starts_with("/whoami") {
            // Ambient caller identity as the plugin sees it, both halves
            // ("{workload-id}|{component-id}").
            let id = store::whoami().await;
            return Ok(make_response(200, id.into_bytes()));
        }
        if route.starts_with("/dropped-buckets") {
            let n = store::dropped_buckets().await;
            return Ok(make_response(200, n.to_string().into_bytes()));
        }
        if route.starts_with("/begin") {
            // Long-running, cancellable plugin call. A cancel is cooperative: the
            // plugin observes it on its next tick and returns early, so this
            // request still gets a normal response carrying the partial tick
            // count; `/progress` observes how far it got independently.
            let name = query_get(query, "name").unwrap_or_default();
            let ticks: u64 = query_get(query, "ticks")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let tick_ms: u64 = query_get(query, "tick-ms")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let reached = store::begin(name, ticks, tick_ms).await;
            return Ok(make_response(200, reached.to_string().into_bytes()));
        }
        if route.starts_with("/cancel-job") {
            let name = query_get(query, "name").unwrap_or_default();
            let ok = store::cancel_job(name).await;
            return Ok(make_response(200, if ok { b"true".to_vec() } else { b"false".to_vec() }));
        }
        if route.starts_with("/progress") {
            let name = query_get(query, "name").unwrap_or_default();
            let p = store::progress(name).await;
            return Ok(make_response(200, p.to_string().into_bytes()));
        }

        Ok(make_response(404, Vec::new()))
    }
}

/// Return the value of `name` from a `k=v&k2=v2` query string, if present.
fn query_get(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

fn make_response(status: u16, body: Vec<u8>) -> Response {
    let headers = Fields::new();
    let _ = headers.set(&"content-type".to_string(), &[b"application/octet-stream".to_vec()]);
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    wit_bindgen::spawn_local(async move {
        tx.write_all(body).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    let _ = response.set_status_code(status);
    response
}

mod export {
    #![allow(unsafe_code)]
    use super::{bindings, Component};
    bindings::export!(Component with_types_in bindings);
}
