//! A p3 proxy service: `cli/run` parks forever while `http/handler` serves
//! ingress on the same long-lived instance.
//!
//! A request carrying an `x-backend` header (an `authority` such as
//! `127.0.0.1:8080`) is forwarded to that authority through the imported
//! `wasi:http/client` — the host's outbound HTTP path — and the backend's
//! response resource is returned unchanged, so its body streams through
//! without a guest-side copy. A request without the header is answered
//! directly from the service; a present-but-malformed header is an error,
//! never a silent fallback to the (much faster) direct path.

mod bindings {
    wit_bindgen::generate!({ world: "svc-http-proxy", generate_all });
}

use bindings::exports::wasi::cli::run::Guest as RunGuest;
use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::http::client as outbound;
use bindings::wasi::http::types::{ErrorCode, Fields, Method, Request, Response, Scheme};

struct Component;

impl RunGuest for Component {
    async fn run() -> Result<(), ()> {
        use bindings::wasi::clocks::monotonic_clock;
        // The run loop has no work of its own; it only has to stay alive so
        // the trigger driver keeps co-driving the handler. No periodic tick:
        // each in-flight HTTP job re-enters the store and drives the
        // instance's tasks itself, and a tick would only add wakeups that
        // show up as latency outliers in the benchmarks.
        loop {
            monotonic_clock::wait_for(u64::MAX).await;
        }
    }
}

impl HttpGuest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        match request
            .get_headers()
            .get(&"x-backend".to_string())
            .into_iter()
            .next()
        {
            Some(value) => {
                let authority = String::from_utf8(value)
                    .map_err(|_| internal("x-backend header is not UTF-8"))?;
                proxy(&authority).await
            }
            None => Ok(direct_response()),
        }
    }
}

fn internal(msg: &str) -> ErrorCode {
    ErrorCode::InternalError(Some(msg.to_string()))
}

/// Forward a `GET /` to `authority` over the imported client and return the
/// backend's response as-is.
async fn proxy(authority: &str) -> Result<Response, ErrorCode> {
    let headers = Fields::new();
    // No request body: dropping the unwritten trailers writer resolves the
    // future with the default `Ok(None)` as soon as the host polls it.
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    drop(trailers_tx);
    let (request, _sent) = Request::new(headers, None, trailers_rx, None);
    request
        .set_method(&Method::Get)
        .map_err(|()| internal("set_method rejected GET"))?;
    request
        .set_scheme(Some(&Scheme::Http))
        .map_err(|()| internal("set_scheme rejected http"))?;
    request
        .set_authority(Some(authority))
        .map_err(|()| internal("set_authority rejected the x-backend value"))?;
    request
        .set_path_with_query(Some("/"))
        .map_err(|()| internal("set_path_with_query rejected /"))?;
    outbound::send(request).await
}

fn direct_response() -> Response {
    let headers = Fields::new();
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    wit_bindgen::spawn_local(async move {
        tx.write_all(b"hello from service".to_vec()).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    response
}

bindings::export!(Component with_types_in bindings);
