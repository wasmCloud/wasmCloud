//! Fixture exercising core NATS delivery and subject policy.
//!
//! Each delivery echoes onto a result subject so the test can observe what the
//! guest saw. A body prefixed with `denied:` makes the handler publish outside
//! its grant, so the denial is visible to the test as a returned error rather
//! than as a message that never arrives.

wit_bindgen::generate!({
    path: "wit",
    world: "nats-core-handler-fixture",
    generate_all,
});

use exports::wasmcloud::nats::core_handler::Guest;
use wasmcloud::nats::core;
use wasmcloud::nats::types::{NatsError, NatsMessage};

/// Bodies starting with this make the handler fail, so the error path is
/// observable. Core NATS has no ack, so this only surfaces in host logs.
const FAIL_MARKER: &str = "fail:";
/// Bodies starting with this make the handler reach outside its grant.
const DENIED_MARKER: &str = "denied:";

/// Where the handler echoes what it saw.
const RESULT_SUBJECT: &str = "test.results";

struct Component;

fn label(err: &NatsError) -> String {
    match err {
        NatsError::SubjectDenied(subject) => format!("subject-denied:{subject}"),
        NatsError::MaxPayloadExceeded(limit) => format!("max-payload-exceeded:{limit}"),
        NatsError::NoResponders => "no-responders".to_string(),
        NatsError::Timeout(d) => format!("timeout:{d}"),
        NatsError::Connection(d) => format!("connection:{d}"),
        _ => "other".to_string(),
    }
}

fn publish(subject: &str, body: &str) -> Result<(), NatsError> {
    core::publish(&NatsMessage {
        subject: subject.to_string(),
        reply_to: None,
        body: body.as_bytes().to_vec(),
        headers: None,
    })
}

impl Guest for Component {
    fn handle_message(msg: NatsMessage) -> Result<(), String> {
        let body = String::from_utf8_lossy(&msg.body).to_string();

        // Reaching outside the grant must come back as a typed denial the
        // guest can act on, not as a message that silently never arrives.
        if let Some(subject) = body.strip_prefix(DENIED_MARKER) {
            let outcome = match publish(subject, "should never arrive") {
                Ok(()) => "unexpectedly-allowed".to_string(),
                Err(e) => label(&e),
            };
            let _ = publish(RESULT_SUBJECT, &outcome);
            return Ok(());
        }

        let _ = publish(RESULT_SUBJECT, &format!("{}|{}", msg.subject, body));

        if body.starts_with(FAIL_MARKER) {
            return Err(format!("handler refused: {body}"));
        }
        Ok(())
    }
}

export!(Component);
