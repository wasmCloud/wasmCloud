//! Fixture exercising JetStream delivery, auto-ack, KV, and subject policy.
//!
//! Each delivery echoes onto a result subject so the test can observe what the
//! guest saw, then records the delivery count in KV so redelivery is visible.

wit_bindgen::generate!({
    path: "wit",
    world: "nats-jetstream-handler-fixture",
    generate_all,
});

use exports::wasmcloud::nats::jetstream_handler::Guest;
use wasmcloud::nats::jetstream::{self, MessageHandle};
use wasmcloud::nats::kv;
use wasmcloud::nats::types::{NatsError, NatsMessage};

/// Bodies starting with this make the handler fail, to exercise auto-nak.
const FAIL_MARKER: &str = "fail:";
/// Bodies starting with this make the handler reach outside its grant.
const DENIED_MARKER: &str = "denied:";

struct Component;

fn label(err: &NatsError) -> String {
    match err {
        NatsError::SubjectDenied(subject) => format!("subject-denied:{subject}"),
        NatsError::MaxPayloadExceeded(limit) => format!("max-payload-exceeded:{limit}"),
        NatsError::NoResponders => "no-responders".to_string(),
        NatsError::UnsupportedByServer(detail) => format!("unsupported-by-server:{detail}"),
        NatsError::NotFound(what) => format!("not-found:{what}"),
        NatsError::KeyNotFound => "key-not-found".to_string(),
        NatsError::RevisionMismatch(rev) => format!("revision-mismatch:{rev}"),
        NatsError::NoMessages => "no-messages".to_string(),
        NatsError::Disconnected => "disconnected".to_string(),
        NatsError::Timeout(d) => format!("timeout:{d}"),
        NatsError::Connection(d) => format!("connection:{d}"),
        NatsError::Jetstream(d) => format!("jetstream:{d}"),
        NatsError::Unexpected(d) => format!("unexpected:{d}"),
    }
}

fn publish(subject: &str, body: String) -> Result<(), NatsError> {
    jetstream::publish(&NatsMessage {
        subject: subject.to_string(),
        body: body.into_bytes(),
        reply_to: None,
        headers: None,
    })
    .map(|_| ())
}

impl Guest for Component {
    fn handle_message(handle: MessageHandle) -> Result<(), String> {
        let message = handle.message();
        let body = String::from_utf8_lossy(&message.body).to_string();
        let delivery = handle.delivery_count();

        // Reaching a subject outside the grant must fail in the host, not at
        // the server, and must name the refused subject.
        if let Some(target) = body.strip_prefix(DENIED_MARKER) {
            let outcome = match publish(target, "should not arrive".to_string()) {
                Ok(()) => "allowed".to_string(),
                Err(e) => label(&e),
            };
            let _ = publish("test.results", format!("denied-probe:{outcome}"));
            return Ok(());
        }

        // Record how many times this body has been delivered, so the test can
        // tell a redelivery from a fresh message.
        if let Ok(bucket) = kv::open("test-counts") {
            let key = body.replace(['.', ':'], "_");
            let next = match bucket.get(&key) {
                Ok(Some(entry)) => {
                    let prev: u32 = String::from_utf8_lossy(&entry.value).parse().unwrap_or(0);
                    let _ = bucket.update(&key, prev.saturating_add(1).to_string().as_bytes(), entry.revision);
                    prev + 1
                }
                _ => {
                    let _ = bucket.create(&key, b"1");
                    1
                }
            };
            let _ = publish("test.results", format!("count:{body}:{next}"));
        }

        if body.starts_with(FAIL_MARKER) {
            // Under ack-mode auto this naks, so the message comes back.
            return Err(format!("deliberate failure on delivery {delivery}"));
        }

        publish("test.results", format!("handled:{body}:delivery={delivery}"))
            .map_err(|e| label(&e))?;
        Ok(())
    }
}

export!(Component);
