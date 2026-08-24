//! Two named `wasmcloud:nats/core` imports, one component.
//!
//! A message arriving on either binding's subscription is republished to both
//! `hub` and `leaf`, so a test can see that each label reached a different
//! server — the routing the host does per `(implements ..)` name.

wit_bindgen::generate!({
    path: "wit",
    world: "nats-implements-p3",
    generate_all,
});

use exports::wasmcloud::nats::core_handler::Guest as CoreGuest;
use wasi::clocks::monotonic_clock;
use wasmcloud::nats::types::{NatsError, NatsMessage};

/// Keeps the component genuinely P3: a handler that awaits no `@0.3` import
/// lets that import tree-shake away.
const YIELD_NANOS: u64 = 1_000_000;

struct Component;

fn message(subject: &str, body: String) -> NatsMessage {
    NatsMessage {
        subject: subject.to_string(),
        body: body.into_bytes(),
        reply_to: None,
        headers: None,
    }
}

fn label(err: &NatsError) -> String {
    match err {
        NatsError::Denied(d) => format!("denied:{}", d.name),
        NatsError::Disconnected => "disconnected".to_string(),
        NatsError::Timeout(d) => format!("timeout:{d}"),
        NatsError::Connection(d) => format!("connection:{d}"),
        other => format!("error:{other:?}"),
    }
}

impl CoreGuest for Component {
    async fn handle_message(msg: NatsMessage) -> Result<(), String> {
        monotonic_clock::wait_for(YIELD_NANOS).await;
        let body = String::from_utf8_lossy(&msg.body).to_string();

        // One publish per binding. Only routing decides which server each
        // lands on: the subjects and the payloads are identical.
        hub::publish(message("bridge.hub", format!("hub:{body}")))
            .await
            .map_err(|e| format!("hub publish failed: {}", label(&e)))?;
        leaf::publish(message("bridge.leaf", format!("leaf:{body}")))
            .await
            .map_err(|e| format!("leaf publish failed: {}", label(&e)))
    }
}

export!(Component);
