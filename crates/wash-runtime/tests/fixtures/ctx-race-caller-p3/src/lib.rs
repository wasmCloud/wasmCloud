//! Caller for the `active_ctx` race tests. Every route reports which
//! component's context the caller's OWN host calls resolved to: its `WHO`
//! environment variable, and whether an outgoing request to `example.com` is
//! permitted (502 from the stub upstream) or denied by policy (403). The caller
//! is granted no hosts; the linked callee is granted `example.com`.
//!
//! - `/whoami`: no linked call at all.
//! - `/preopens`: the caller's own preopened directories.
//! - `/inflight`: one linked call parked in the callee while the caller keeps
//!   running.
//! - `/overlap`: two linked calls in flight at once, finished first-in
//!   first-out, then the caller reads its context after both returned.

mod bindings {
    wit_bindgen::generate!({ world: "ctx-race-caller", generate_all });
}

use std::pin::pin;

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::cli::environment::get_environment;
use bindings::wasi::http::client::send;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response, Scheme};
use bindings::wasmcloud::ctx_race::gate;

struct Component;

fn who() -> String {
    get_environment()
        .into_iter()
        .find(|(k, _)| k == "WHO")
        .map(|(_, v)| v)
        .unwrap_or_else(|| "?".into())
}

async fn egress() -> u16 {
    match fetch("example.com", "/").await {
        Ok(_) => 200,
        Err(ErrorCode::HttpRequestDenied) => 403,
        Err(_) => 502,
    }
}

async fn fetch(authority: &str, path: &str) -> Result<u16, ErrorCode> {
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    let (request, _sent) = Request::new(Fields::new(), None, trailers_rx, None);
    wit_bindgen::spawn_local(async move {
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let set = |r: Result<(), ()>| r.map_err(|()| ErrorCode::InternalError(None));
    set(request.set_scheme(Some(&Scheme::Http)))?;
    set(request.set_authority(Some(authority)))?;
    set(request.set_path_with_query(Some(path)))?;
    let response = send(request).await?;
    Ok(response.get_status_code())
}

async fn snapshot() -> String {
    format!("{{\"who\":\"{}\",\"egress\":{}}}", who(), egress().await)
}

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let body = match request.get_path_with_query().unwrap_or_default().as_str() {
            "/whoami" => snapshot().await,
            // This component's own preopened directories, `+`-joined.
            "/preopens" => format!(
                "{{\"preopens\":\"{}\"}}",
                bindings::wasi::filesystem::preopens::get_directories()
                    .into_iter()
                    .map(|(_, p)| p)
                    .collect::<Vec<_>>()
                    .join("+")
            ),
            "/inflight" => {
                let (tx, rx) = bindings::wit_future::new::<u32>(|| 0);
                let mut call = pin!(gate::hold(rx));
                // One poll starts the linked call; it parks in the callee.
                assert!(futures::poll!(call.as_mut()).is_pending());
                let during = snapshot().await;
                let _ = tx.write(1).await;
                let callee = call.await;
                let after = snapshot().await;
                format!(
                    "{{\"during\":{during},\"callee\":\"{callee}\",\"after\":{after}}}"
                )
            }
            "/overlap" => {
                let (tx1, rx1) = bindings::wit_future::new::<u32>(|| 0);
                let (tx2, rx2) = bindings::wit_future::new::<u32>(|| 0);
                let mut c1 = pin!(gate::hold(rx1));
                let mut c2 = pin!(gate::hold(rx2));
                assert!(futures::poll!(c1.as_mut()).is_pending());
                assert!(futures::poll!(c2.as_mut()).is_pending());
                // Finish the FIRST call first, then the second.
                let _ = tx1.write(1).await;
                let r1 = c1.await;
                let _ = tx2.write(2).await;
                let r2 = c2.await;
                let after = snapshot().await;
                format!("{{\"r1\":\"{r1}\",\"r2\":\"{r2}\",\"after\":{after}}}")
            }
            _ => "{}".to_string(),
        };
        Ok(respond(200, body.into_bytes()))
    }
}

fn respond(status: u16, body: Vec<u8>) -> Response {
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    wit_bindgen::spawn_local(async move {
        tx.write_all(body).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(Fields::new(), Some(rx), trailers_rx);
    let _ = response.set_status_code(status);
    response
}

bindings::export!(Component with_types_in bindings);
