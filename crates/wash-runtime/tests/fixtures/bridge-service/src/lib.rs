//! Long-lived p3 service: HTTP ingress + co-driven cli/run. Each request calls
//! the stateless `wasmcloud:bridge/ops` backend across the host bridge and
//! returns the result as JSON. Path selects the operation:
//!
//! - `/add`     -> `ops.add(40, 2)`            handle-free call (fast path)
//! - `/bump`    -> `ops.bump()`                always 1 (backend is stateless)
//! - `/consume` -> `ops.consume(stream<u8>)`   service -> component stream
//! - `/produce` -> `ops.produce(n)`            component -> service stream
//! - `/sum`     -> `ops.sum(stream<u32>)`      non-u8 element type
//! - `/relay`   -> `ops.relay(variant data(stream<u8>))`  nested-in-variant
//! - `/delayed` -> `ops.delayed(v)`            component -> service `future<u64>`

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "service", generate_all });
}

use bindings::exports::wasi::cli::run::Guest as RunGuest;
use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::bridge::ops;

struct Component;

impl HttpGuest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request
            .get_path_with_query()
            .unwrap_or_else(|| "/".to_string());

        let body = if path.starts_with("/add") {
            let v = ops::add(40, 2).await;
            format!("{{\"result\":{v},\"expected\":42}}")
        } else if path.starts_with("/bump") {
            let v = ops::bump().await;
            format!("{{\"result\":{v},\"expected\":1}}")
        } else if path.starts_with("/block") {
            // Calls a backend that busy-spins (monopolizing its own store). The
            // await yields the service's HTTP task, so the service stays free to
            // serve other requests concurrently — unless the backend shared the
            // service's store, in which case the spin would freeze it.
            let _acc = ops::block(300_000_000).await;
            "{\"result\":1,\"expected\":1}".to_string()
        } else if path.starts_with("/produce-drop-early") {
            // Read only the first chunk of a large component-produced stream,
            // then drop the reader. The backend's writer side must unwind
            // cleanly (no trap) when its consumer goes away.
            let full: u64 = 70_000;
            let mut rx = ops::produce(full).await;
            let (_result, chunk) = rx.read(Vec::with_capacity(1024)).await;
            let read = chunk.len() as u64;
            drop(rx);
            format!("{{\"result\":{read},\"expected\":{full}}}")
        } else if path.starts_with("/produce") {
            let expected: u64 = 70_000;
            let mut rx = ops::produce(expected).await;
            let mut total: u64 = 0;
            loop {
                let (result, chunk) = rx.read(Vec::with_capacity(4096)).await;
                total += chunk.len() as u64;
                if matches!(result, wit_bindgen::StreamResult::Dropped) {
                    break;
                }
            }
            format!("{{\"result\":{total},\"expected\":{expected}}}")
        } else if path.starts_with("/sum") {
            let (mut tx, rx) = bindings::wit_stream::new();
            wit_bindgen::spawn_local(async move {
                for i in 0..1000u32 {
                    tx.write_all(vec![i]).await;
                }
                drop(tx);
            });
            let v = ops::sum(rx).await;
            format!("{{\"result\":{v},\"expected\":499500}}")
        } else if path.starts_with("/delayed") {
            // The backend returns a `future<u64>`; the bridge relocates it across
            // the store boundary and the service awaits its resolved value.
            let expected: u64 = 12345;
            let reader = ops::delayed(expected).await;
            let v = reader.await;
            format!("{{\"result\":{v},\"expected\":{expected}}}")
        } else if path.starts_with("/relay") {
            let (mut tx, rx) = bindings::wit_stream::new();
            wit_bindgen::spawn_local(async move {
                for _ in 0..1000 {
                    tx.write_all(vec![b'z'; 50]).await;
                }
                drop(tx);
            });
            let v = ops::relay(ops::Chunked::Data(rx)).await;
            format!("{{\"result\":{v},\"expected\":50000}}")
        } else if path.starts_with("/consume-large") {
            // ~4 MiB through the bounded relocation channel (capacity is a
            // handful of chunks): must complete without buffering the whole
            // stream — the channel back-pressures the writer.
            const CHUNK_LEN: usize = 1024;
            const CHUNKS: u64 = 4096; // 4 MiB
            let expected = CHUNK_LEN as u64 * CHUNKS;
            let (mut tx, rx) = bindings::wit_stream::new();
            wit_bindgen::spawn_local(async move {
                let chunk = vec![b'q'; CHUNK_LEN];
                for _ in 0..CHUNKS {
                    tx.write_all(chunk.clone()).await;
                }
                drop(tx);
            });
            let v = ops::consume(rx).await;
            format!("{{\"result\":{v},\"expected\":{expected}}}")
        } else {
            // `/consume`
            const CHUNK: &[u8] = b"hello world from the service........0123456789AB\n"; // 49 bytes
            const CHUNKS: usize = 1234;
            let (mut tx, rx) = bindings::wit_stream::new();
            wit_bindgen::spawn_local(async move {
                for _ in 0..CHUNKS {
                    tx.write_all(CHUNK.to_vec()).await;
                }
                drop(tx);
            });
            let v = ops::consume(rx).await;
            let expected = (CHUNK.len() * CHUNKS) as u64;
            format!("{{\"result\":{v},\"expected\":{expected}}}")
        };

        Ok(make_response(200, body.into_bytes()))
    }
}

impl RunGuest for Component {
    async fn run() -> Result<(), ()> {
        use bindings::wasi::clocks::monotonic_clock;
        // Keep the service alive; the co-driver runs this concurrently with HTTP.
        loop {
            monotonic_clock::wait_for(1_000_000).await;
        }
    }
}

fn make_response(status: u16, body: Vec<u8>) -> Response {
    let headers = Fields::new();
    let _ = headers.set(&"content-type".to_string(), &[b"application/json".to_vec()]);
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
