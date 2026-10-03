//! P3 fixture (HTTP entrypoint): obtains a `token` from `res-producer-p3` and
//! hands it to `res-sink-p3`, then returns the sink's reply as the HTTP body.
//! Passing the token to `accept` is what exercises `lower_with_type`'s
//! resource-identity passthrough across the dynamic linker.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
        async: [
            "import:wasmcloud:resource-test/factory@0.1.0#make-token",
            "import:wasmcloud:resource-test/sink@0.1.0#accept",
            "import:wasmcloud:resource-test/sink@0.1.0#bounce",
            "export:wasi:http/handler@0.3.0#handle",
        ],
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::resource_test::{factory, sink};

struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let body = if request.get_path_with_query().as_deref() == Some("/drop") {
            for _ in 0..128 {
                let token = factory::make_token("direct".into()).await;
                let before = factory::stats();
                assert_eq!(token.greet(), "hello direct");
                assert_eq!(
                    factory::stats(),
                    before,
                    "borrowing must not destroy the resource"
                );
                drop(token);

                let token = factory::make_token("sink".into()).await;
                assert_eq!(sink::accept(token).await, "sink:hello sink");

                let token = factory::make_token("bounce".into()).await;
                let token = sink::bounce(token).await;
                assert_eq!(token.greet(), "hello bounce");
                drop(token);
            }
            let environment = bindings::wasi::cli::environment::get_environment();
            assert!(environment
                .iter()
                .any(|(k, v)| k == "RESOURCE_DROP_CONTEXT" && v == "res-caller"));
            let (created, dropped, wrong_context) = factory::stats();
            format!("{created},{dropped},{wrong_context}")
        } else {
            // The owned handle crosses both linker hops before the sink drops it.
            let token = factory::make_token("world".to_string()).await;
            sink::accept(token).await
        };

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
