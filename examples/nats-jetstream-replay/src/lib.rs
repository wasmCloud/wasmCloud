//! Order processing over JetStream, with replay.
//!
//! Two paths share one component. The host pushes new deliveries into
//! `handle-message`, and the same component can replay history from an
//! arbitrary stream sequence with `scan`.

wit_bindgen::generate!({
    path: "wit",
    world: "nats-jetstream-replay",
    generate_all,
});

use exports::wasmcloud::nats::jetstream_handler::Guest as JetstreamHandler;
use wasmcloud::nats::jetstream::{self, MessageHandle};
use wasmcloud::nats::kv;
use wasmcloud::nats::types::{HeaderEntry, NatsError, NatsMessage};

/// Bucket holding the running per-order totals.
const TOTALS_BUCKET: &str = "order-totals";
/// Subject the processed-order notification is published to.
const PROCESSED_SUBJECT: &str = "orders.processed";

struct Component;

/// Renders an error for the handler's `result<_, string>`.
fn describe(err: &NatsError) -> String {
    match err {
        NatsError::Connection(detail) => format!("connection: {detail}"),
        NatsError::Timeout(detail) => format!("timeout: {detail}"),
        NatsError::NoResponders => "no responders on subject".to_string(),
        NatsError::SubjectDenied(subject) => {
            format!("subject `{subject}` is outside this workload's grant")
        }
        NatsError::MaxPayloadExceeded(limit) => format!("payload exceeds server limit {limit}"),
        NatsError::Jetstream(detail) => format!("jetstream: {detail}"),
        NatsError::KeyNotFound => "key not found".to_string(),
        NatsError::RevisionMismatch(current) => format!("revision mismatch, current {current}"),
        NatsError::NoMessages => "no messages".to_string(),
        NatsError::NotFound(what) => format!("not found: {what}"),
        NatsError::UnsupportedByServer(detail) => format!("unsupported by server: {detail}"),
        NatsError::Disconnected => "disconnected".to_string(),
        NatsError::Unexpected(detail) => format!("unexpected: {detail}"),
    }
}

/// Adds an order's amount to the running total, retrying on a CAS conflict.
///
/// `update` is compare-and-swap on revision, and a mismatch carries the current
/// revision, so a retry does not need a re-read. Two concurrent handlers on the
/// same key therefore converge instead of silently losing a write, which is
/// what a last-write-wins `put` would do here.
fn accumulate(bucket: &kv::Bucket, key: &str, amount: u64) -> Result<u64, NatsError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let current = bucket.get(key)?;
        let (running, revision) = match &current {
            Some(entry) => (read_u64(&entry.value).unwrap_or(0), entry.revision),
            None => (0, 0),
        };
        let next = running.saturating_add(amount);
        let encoded = next.to_string().into_bytes();

        let result = if current.is_some() {
            bucket.update(key, &encoded, revision)
        } else {
            bucket.create(key, &encoded)
        };

        match result {
            Ok(_) => return Ok(next),
            // Someone else wrote between the read and the write. Re-read and
            // reapply rather than clobbering their value.
            Err(NatsError::RevisionMismatch(_)) if attempt < 5 => continue,
            Err(e) => return Err(e),
        }
    }
}

fn read_u64(raw: &[u8]) -> Option<u64> {
    std::str::from_utf8(raw).ok()?.trim().parse().ok()
}

/// Reads `order-id:amount` out of a message body.
fn parse_order(body: &[u8]) -> Option<(String, u64)> {
    let text = std::str::from_utf8(body).ok()?;
    let (id, amount) = text.trim().split_once(':')?;
    if id.is_empty() {
        return None;
    }
    Some((id.to_string(), amount.trim().parse().ok()?))
}

impl JetstreamHandler for Component {
    /// Processes one delivery.
    ///
    /// Delivery is at-least-once: a redelivery after a partial failure calls
    /// this again with the same body, so the work must be idempotent. The
    /// `delivery-count` check below is what makes the retry visible rather
    /// than silent.
    fn handle_message(handle: MessageHandle) -> Result<(), String> {
        let message = handle.message();
        let sequence = handle.sequence();
        let delivery = handle.delivery_count();

        let Some((order_id, amount)) = parse_order(&message.body) else {
            // A malformed body will never parse, however many times it is
            // redelivered, so reject it permanently instead of returning an
            // error and having it come back forever.
            let _ = handle.term();
            return Err(format!(
                "order at sequence {sequence} is not `order-id:amount`"
            ));
        };

        if delivery > 1 {
            println!("order {order_id} redelivered (attempt {delivery}), reapplying idempotently");
        }

        let bucket = kv::open(TOTALS_BUCKET).map_err(|e| describe(&e))?;
        let running = accumulate(&bucket, &order_id, amount).map_err(|e| describe(&e))?;

        // `Nats-Msg-Id` deduplicates within the stream's duplicate window, so a
        // redelivered order does not publish a second notification. The window
        // is stream configuration, not a guarantee the interface makes.
        let notification = NatsMessage {
            subject: PROCESSED_SUBJECT.to_string(),
            body: format!("{order_id}:{running}").into_bytes(),
            reply_to: None,
            headers: Some(vec![HeaderEntry {
                name: "Nats-Msg-Id".to_string(),
                value: format!("processed-{order_id}-{sequence}"),
            }]),
        };
        let ack = jetstream::publish(&notification).map_err(|e| describe(&e))?;

        println!(
            "order {order_id} +{amount} -> {running} (stream seq {}, duplicate: {})",
            ack.sequence, ack.duplicate
        );

        // Returning Ok is what acks under `ack-mode: auto`. Under `manual` the
        // handler would call `handle.ack()` itself.
        Ok(())
    }
}

export!(Component);
