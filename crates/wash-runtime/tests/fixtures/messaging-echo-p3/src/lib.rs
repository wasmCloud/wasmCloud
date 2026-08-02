//! Real-guest fixture for the async `wasmcloud:messaging@0.3.0` surface.
//!
//! The async counterpart of the `messaging-echo` fixture. `handle-message` is an
//! `async fn` that awaits an imported `consumer::publish` to reply on the
//! message's `reply-to` subject — so one message exercises both directions of
//! the async ABI: the host invoking an exported `async func`, and the guest
//! awaiting an imported one from inside it.
//!
//! Each handled message also bumps a process-global `MSG_COUNT`, which the
//! `wasi:http/handler` export reports as `{"count":N}`. That makes delivery
//! observable end-to-end: a test publishes through a backend, then reads the
//! count back over HTTP.
//!
//! It runs as a service, so the trigger service keeps ONE long-lived instance.
//! That is what makes the count observable: a per-message component would get a
//! fresh linear memory per message and `MSG_COUNT` would always read back as
//! zero. `wasi:cli/run` is exported because the host's bind path for this
//! workload shape still expects it, but it has no work to do and returns
//! immediately — the instance is held open by the ingress serve loops.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::bindings::exports::wasi::cli::run::Guest as RunGuest;
use crate::bindings::exports::wasi::http::handler::Guest as HttpGuest;
use crate::bindings::exports::wasmcloud::messaging::handler::Guest as MsgGuest;
use crate::bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use crate::bindings::wasmcloud::messaging::{
    consumer,
    types::{BrokerMessage, Error},
};

mod bindings {
    use crate::Component;

    wit_bindgen::generate!({
        world: "echo-p3",
        generate_all
    });

    export!(Component);
}

static MSG_COUNT: AtomicU64 = AtomicU64::new(0);

struct Component;

impl RunGuest for Component {
    async fn run() -> Result<(), ()> {
        // Nothing to co-drive: this fixture's work is entirely in its handler
        // exports. Returning immediately does NOT end the service — the trigger
        // service holds `run_concurrent` open on the ingress serve loops, and
        // `cli/run` returning is only logged. The instance (and so `MSG_COUNT`)
        // survives across messages either way.
        Ok(())
    }
}

/// Where the echo goes when the inbound message carries no `reply-to`. Nothing
/// subscribes to it, so the publish resolves against zero subscribers — enough
/// to exercise the async import without feeding the handler its own output.
const SINK_SUBJECT: &str = "echo.sink";

impl MsgGuest for Component {
    async fn handle_message(msg: BrokerMessage) -> Result<(), Error> {
        // Awaiting an imported `async func` from inside an exported one is the
        // point of this fixture: under `@0.2.0` this reply was a blocking call.
        // The async ABI passes records by value, unlike the sync bindings which
        // take `&BrokerMessage`.
        consumer::publish(BrokerMessage {
            subject: msg.reply_to.unwrap_or_else(|| SINK_SUBJECT.to_string()),
            body: msg.body,
            reply_to: None,
        })
        .await?;

        // Counted only AFTER the awaited publish resolves, so an observed count
        // is evidence the async import completed — not merely that the exported
        // handler was entered.
        MSG_COUNT.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

impl HttpGuest for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let count = MSG_COUNT.load(Ordering::SeqCst);
        Ok(make_response(200, format!("{{\"count\":{count}}}").into_bytes()))
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
