//! Caller for the `active_ctx` race tests. Every route reports which
//! component's context the caller's OWN host calls resolved to: its `WHO`
//! environment variable, and whether an outgoing request to `example.com` is
//! permitted (502 from the stub upstream) or denied by policy (403). The caller
//! is granted no hosts; the linked callee is granted `example.com`.
//!
//! - `/whoami`: no linked call at all.
//! - `/inflight`: one linked call parked in the callee while the caller keeps
//!   running.
//! - `/overlap`: two linked calls in flight at once, finished first-in
//!   first-out, then the caller reads its context after both returned.

mod bindings {
    wit_bindgen::generate!({ world: "ctx-race-nest-caller", generate_all });
}

use std::pin::pin;

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::cli::environment::get_environment;
use bindings::wasi::http::client::send;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response, Scheme};
use bindings::wasmcloud::ctx_race::relay;

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
            "/preopens" => bindings::wasi::filesystem::preopens::get_directories()
                .into_iter()
                .map(|(_, p)| p)
                .collect::<Vec<_>>()
                .join("+")
                .pipe_json(),
            "/nested" => {
                let (tx, rx) = bindings::wit_future::new::<u32>(|| 0);
                let mut call = pin!(relay::hold(rx));
                assert!(futures::poll!(call.as_mut()).is_pending());
                let during = snapshot().await;
                let _ = tx.write(1).await;
                let b = call.await;
                let after = snapshot().await;
                format!("{{\"during\":{during},\"b\":{b},\"after\":{after}}}")
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

trait PipeJson {
    fn pipe_json(self) -> String;
}
impl PipeJson for String {
    fn pipe_json(self) -> String {
        format!("{{\"preopens\":\"{self}\"}}")
    }
}
