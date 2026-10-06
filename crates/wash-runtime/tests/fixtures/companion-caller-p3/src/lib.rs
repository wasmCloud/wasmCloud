//! P3 fixture, the HTTP entrypoint for the companion-store tests. Each route
//! drives one scenario against the `token` owner and reports what it saw as
//! the response body.

use futures::future::{join, poll_immediate};

mod bindings {
    wit_bindgen::generate!({
        world: "caller",
        generate_all,
        async: [
            "import:wasmcloud:companion-test/tokens@0.1.0#[method]token.wait",
            "import:wasmcloud:companion-test/tokens@0.1.0#[method]token.signal",
            "import:wasmcloud:companion-test/tokens@0.1.0#make-held",
            "import:wasmcloud:companion-test/adopter@0.1.0#adopt-after",
            "import:wasi:clocks/monotonic-clock@0.3.0#wait-for",
            "export:wasi:http/handler@0.3.0#handle",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::clocks::monotonic_clock;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::companion_test::adopter;
use bindings::wasmcloud::companion_test::tokens::{self, Token};

struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let body = match request.get_path_with_query().unwrap_or_default().as_str() {
            "/overlap" => overlap().await,
            "/cancel" => cancel().await,
            "/deferred-drop" => deferred_drop().await,
            other => format!("unknown route {other}"),
        };
        Ok(respond(body))
    }
}

/// Two calls in flight on one token at once. `wait` is sent first and only
/// returns once `signal` has run, so an owner that took them in turn would
/// never answer.
async fn overlap() -> String {
    let token = Token::new("overlap");
    let (message, ()) = join(token.wait(), token.signal("signalled".to_string())).await;
    message
}

/// Abandon a call after the owner has started it. The token that call goes on
/// to return has nowhere to go, and the owner has to drop it.
async fn cancel() -> String {
    let mut held = Box::pin(tokens::make_held("held".to_string()));
    let started = poll_immediate(&mut held).await.is_none();
    let created = live_becomes(1).await;
    drop(held);
    tokens::release();
    let dropped = live_becomes(0).await;
    format!("started={started} created={created} dropped={dropped}")
}

/// Have the adopter drop a token while the owner is inside a sync call. The
/// destructor cannot run until that call returns, and has to run then.
async fn deferred_drop() -> String {
    let ready = adopter::ready();
    let token = Token::new("adopted");
    let mut adopting = Box::pin(adopter::adopt_after(token, 100));
    let pending = poll_immediate(&mut adopting).await.is_none();
    tokens::hold(600);
    adopting.await;
    let dropped = live_becomes(0).await;
    // The owner is still serving.
    let _after = Token::new("after");
    format!(
        "ready={ready} pending={pending} dropped={dropped} live={}",
        tokens::live()
    )
}

/// Whether the owner's live-token count reaches `want` within two seconds.
async fn live_becomes(want: u32) -> bool {
    for _ in 0..200 {
        if tokens::live() == want {
            return true;
        }
        monotonic_clock::wait_for(10_000_000).await;
    }
    false
}

fn respond(body: String) -> Response {
    let (mut tx, rx) = bindings::wit_stream::new::<u8>();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());
    wit_bindgen::spawn_local(async move {
        tx.write_all(body.into_bytes()).await;
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(Fields::new(), Some(rx), trailers_rx);
    response
}

bindings::export!(Component with_types_in bindings);
