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

/// A key's running total and the stream sequence that last contributed to it.
///
/// Storing the sequence alongside the total is what makes the update
/// idempotent. Adding on every delivery is not: at-least-once means the same
/// order arrives again after any failure downstream of the write, and a bare
/// `total += amount` counts it twice.
struct Total {
    running: u64,
    last_sequence: u64,
}

fn parse_total(raw: &[u8]) -> Option<Total> {
    let text = std::str::from_utf8(raw).ok()?;
    let (running, last_sequence) = text.trim().split_once('@')?;
    Some(Total {
        running: running.trim().parse().ok()?,
        last_sequence: last_sequence.trim().parse().ok()?,
    })
}

fn encode_total(total: &Total) -> Vec<u8> {
    format!("{}@{}", total.running, total.last_sequence).into_bytes()
}

/// Adds an order's amount to the running total, exactly once per sequence.
///
/// `update` is compare-and-swap on revision, and a mismatch carries the current
/// revision, so a retry does not need a re-read. Two concurrent handlers on the
/// same key therefore converge instead of silently losing a write, which is
/// what a last-write-wins `put` would do here.
fn accumulate(
    bucket: &kv::Bucket,
    key: &str,
    amount: u64,
    sequence: u64,
) -> Result<u64, NatsError> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        let current = bucket.get(key)?;
        let (existing, revision) = match &current {
            Some(entry) => (parse_total(&entry.value), entry.revision),
            None => (None, 0),
        };

        // This sequence is already counted, so this is a redelivery. Report the
        // total unchanged rather than adding again.
        if let Some(total) = &existing
            && sequence <= total.last_sequence
        {
            return Ok(total.running);
        }

        let next = Total {
            running: existing
                .as_ref()
                .map_or(0, |t| t.running)
                .saturating_add(amount),
            last_sequence: sequence,
        };
        let encoded = encode_total(&next);

        let result = if current.is_some() {
            bucket.update(key, &encoded, revision)
        } else {
            bucket.create(key, &encoded)
        };

        match result {
            Ok(_) => return Ok(next.running),
            // Someone else wrote between the read and the write. Re-read and
            // reapply rather than clobbering their value.
            Err(NatsError::RevisionMismatch(_)) if attempt < 5 => continue,
            Err(e) => return Err(e),
        }
    }
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
            // A malformed body will never parse, so returning `Err` would nak
            // it and have it redelivered forever. Under `ack-mode: auto` the
            // host owns the acknowledgement and `term()` is not the guest's to
            // call, so report success and let the message be dropped. Bind with
            // `ack-mode: manual` to term it explicitly instead.
            println!("dropping malformed order at sequence {sequence}");
            return Ok(());
        };

        if delivery > 1 {
            println!("order {order_id} redelivered (attempt {delivery})");
        }

        let bucket = kv::open(TOTALS_BUCKET).map_err(|e| describe(&e))?;
        let running =
            accumulate(&bucket, &order_id, amount, sequence).map_err(|e| describe(&e))?;

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
