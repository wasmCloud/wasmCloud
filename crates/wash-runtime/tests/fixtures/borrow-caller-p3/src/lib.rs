//! P3 fixture, the HTTP entrypoint. Creates a `token` in the producer, lends
//! it to the middleware (which lends it on to the consumer), then keeps using
//! it. The body reports the middleware's reply, the caller's own `greet`, and
//! the producer's greet count.

mod bindings {
    wit_bindgen::generate!({
        world: "caller",
        generate_all,
        async: [
            "import:wasmcloud:borrow-test-p3/middleware@0.1.0#accept",
            "import:wasmcloud:borrow-test-p3/middleware@0.1.0#accept-nested",
            "import:wasmcloud:borrow-test-p3/middleware@0.1.0#adopt",
            "export:wasi:http/handler@0.3.0#handle",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::borrow_test_p3::factory::{Lend, Token};
use bindings::wasmcloud::borrow_test_p3::middleware;

struct Component;

impl Handler for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        // `token` is a resource living in the producer; the caller holds the
        // owned handle.
        let token = Token::new("world");
        // Lend it: caller -> middleware -> consumer, where `greet` runs on it.
        let via_middleware = middleware::accept(&token).await;
        // Lend it again from inside a record, a tuple and an option.
        let lend = Lend {
            label: "nested".to_string(),
            token: &token,
        };
        let nested = middleware::accept_nested(lend, ("pair".to_string(), &token), Some(&token)).await;
        // Hand a second token over by value, inside a tuple.
        let adopted = middleware::adopt(("adopted".to_string(), Token::new("owned"))).await;
        // The caller still owns the token and can keep using it.
        let direct = token.greet();
        let body = format!(
            "{via_middleware}|{nested}|{adopted}|{direct}|greets={}",
            token.greets()
        );

        let headers = Fields::new();
        let (mut tx, rx) = bindings::wit_stream::new::<u8>();
        let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| todo!());
        wit_bindgen::spawn_local(async move {
            tx.write_all(body.into_bytes()).await;
            drop(tx);
            let _ = trailers_tx.write(Ok(None)).await;
        });

        let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
        Ok(response)
    }
}

bindings::export!(Component with_types_in bindings);
