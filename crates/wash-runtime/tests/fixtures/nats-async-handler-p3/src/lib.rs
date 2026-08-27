//! Real-guest fixture for the `wasmcloud:nats@0.1.0` surface.
//!
//! Both handlers are `async fn`s that await imported NATS calls from inside
//! the delivery — the shape a sync-signature package cannot express, since a
//! sync export cannot be lifted with the async canonical ABI and cannot await
//! a P3 import.
//!
//! A `wasi:clocks@0.3.0` sleep is awaited before the echo so the component is
//! genuinely P3 — a handler that never awaits a `@0.3` import lets that import
//! tree-shake away, and the result is no longer a P3 component.

wit_bindgen::generate!({
    path: "wit",
    world: "nats-async-handler-fixture",
    generate_all,
});

use exports::wasmcloud::nats::core_handler::Guest as CoreGuest;
use exports::wasmcloud::nats::jetstream_handler::Guest as JsGuest;
use wasi::clocks::monotonic_clock;
use wasmcloud::nats::core;
use wasmcloud::nats::jetstream::{self, MessageHandle};
use wasmcloud::nats::kv;
use wasmcloud::nats::types::{DenialReason, DeniedResource, NatsError, NatsMessage};

/// Bodies starting with this make the handler fail, to exercise auto-nak.
const FAIL_MARKER: &str = "fail:";
/// Bodies starting with this make the handler reach outside its grant.
const DENIED_MARKER: &str = "denied:";
/// How long the handler awaits the P3 clock before echoing.
const YIELD_NANOS: u64 = 1_000_000;
/// KV bucket the delivery counter lives in.
const COUNTS_BUCKET: &str = "test-counts";
/// Bodies starting with this drive a pull-consumer fetch:
/// `pull:<stream>:<consumer>:<batch>[:<max-bytes>]`.
const PULL_MARKER: &str = "pull:";
/// How long a probe fetch waits for its first message.
const PULL_TIMEOUT_MS: u32 = 1_000;
/// Bodies starting with this make the core handler report its in-memory
/// delivery counter, which only survives between messages when the host is
/// reusing the instance (`poolSize`).
const WARM_MARKER: &str = "warm:";

/// Lives in this instance's linear memory and nowhere else: a fresh store per
/// delivery reads 1 every time, a warm instance counts up.
static WARM_DELIVERIES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

struct Component;

fn reason(r: DenialReason) -> &'static str {
    match r {
        DenialReason::Reserved => "reserved",
        DenialReason::NotGranted => "not-granted",
        DenialReason::WildcardNotAllowed => "wildcard-not-allowed",
    }
}

fn target(t: DeniedResource) -> &'static str {
    match t {
        DeniedResource::Subject => "subject",
        DeniedResource::Stream => "stream",
        DeniedResource::Bucket => "bucket",
        DeniedResource::Message => "message",
    }
}

fn label(err: &NatsError) -> String {
    match err {
        NatsError::Denied(d) => {
            format!(
                "denied:{}:{}:{}",
                reason(d.reason),
                target(d.target),
                d.name
            )
        }
        NatsError::MaxPayloadExceeded(limit) => format!("max-payload-exceeded:{limit}"),
        NatsError::InvalidHeader(detail) => format!("invalid-header:{detail}"),
        NatsError::NoResponders => "no-responders".to_string(),
        NatsError::UnsupportedByServer(detail) => format!("unsupported-by-server:{detail}"),
        NatsError::NotFound(what) => format!("not-found:{what}"),
        NatsError::KeyNotFound => "key-not-found".to_string(),
        NatsError::RevisionMismatch(rev) => format!("revision-mismatch:{rev}"),
        NatsError::NoMessages => "no-messages".to_string(),
        NatsError::LimitExceeded(detail) => format!("limit-exceeded:{detail}"),
        NatsError::Disconnected => "disconnected".to_string(),
        NatsError::Timeout(d) => format!("timeout:{d}"),
        NatsError::Connection(d) => format!("connection:{d}"),
        NatsError::Jetstream(d) => format!("jetstream:{d}"),
        NatsError::Unexpected(d) => format!("unexpected:{d}"),
    }
}

fn message(subject: &str, body: String) -> NatsMessage {
    NatsMessage {
        subject: subject.to_string(),
        body: body.into_bytes(),
        reply_to: None,
        headers: None,
    }
}

async fn js_publish(subject: &str, body: String) -> Result<(), NatsError> {
    jetstream::publish(message(subject, body)).await.map(|_| ())
}

impl JsGuest for Component {
    async fn handle_message(handle: MessageHandle) -> Result<(), String> {
        let msg = handle.message();
        let body = String::from_utf8_lossy(&msg.body).to_string();
        let delivery = handle.delivery_count();

        // Awaiting a P3 import from inside the handler is the whole point of
        // the async package; without it `wasi:clocks@0.3.0` tree-shakes away.
        monotonic_clock::wait_for(YIELD_NANOS).await;

        // Report the in-memory delivery counter; see WARM_MARKER.
        if let Some(run) = body.strip_prefix(WARM_MARKER) {
            let seen = WARM_DELIVERIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            return core::publish(message("test.results", format!("warm:{run}:{seen}")))
                .await
                .map_err(|e| label(&e));
        }

        // Reaching a name outside the grant must fail in the host, not at the
        // server, and must say which grant refused it.
        if let Some(subject) = body.strip_prefix(DENIED_MARKER) {
            let outcome =
                match core::publish(message(subject, "should not arrive".to_string())).await {
                    Ok(()) => "allowed".to_string(),
                    Err(e) => label(&e),
                };
            let _ = js_publish("test.results", format!("denied-probe:{outcome}")).await;
            return Ok(());
        }

        // Record how many times this body has been delivered, so the test can
        // tell a redelivery from a fresh message. `get` reports `key-not-found`
        // rather than an empty option in this revision.
        if let Ok(bucket) = kv::open(COUNTS_BUCKET.to_string()).await {
            let key = body.replace(['.', ':'], "_");
            let next = match bucket.get(key.clone()).await {
                Ok(entry) => {
                    let prev: u32 = String::from_utf8_lossy(&entry.value).parse().unwrap_or(0);
                    let _ = bucket
                        .update(
                            key,
                            prev.saturating_add(1).to_string().into_bytes(),
                            entry.revision,
                        )
                        .await;
                    prev + 1
                }
                Err(_) => {
                    let _ = bucket.create(key, b"1".to_vec()).await;
                    1
                }
            };
            let _ = js_publish("test.results", format!("count:{body}:{next}")).await;
        }

        if body.starts_with(FAIL_MARKER) {
            // Under ack-mode auto this naks, so the message comes back.
            return Err(format!("deliberate failure on delivery {delivery}"));
        }

        js_publish(
            "test.results",
            format!("handled:{body}:delivery={delivery}"),
        )
        .await
        .map_err(|e| label(&e))
    }
}

/// Runs one `fetch` against a pull consumer and reports what came back, so a
/// test can see the difference between an idle consumer, a refused request,
/// and a batch cut short by a byte bound.
async fn pull_probe(spec: &str) -> String {
    let parts: Vec<&str> = spec.split(':').collect();
    let [stream, consumer, batch] = parts[..3] else {
        return "pull:bad-spec".to_string();
    };
    let batch: u32 = batch.parse().unwrap_or(1);
    let max_bytes: u64 = parts.get(3).and_then(|b| b.parse().ok()).unwrap_or(0);

    let handle = match jetstream::open_pull_consumer(stream.to_string(), consumer.to_string()).await
    {
        Ok(h) => h,
        Err(e) => return format!("pull:open-failed:{}", label(&e)),
    };

    // The limits the consumer was provisioned with, which is what a guest
    // needs to size a request that will not be refused.
    let limits = match handle.info().await {
        Ok(info) => format!(
            "max-request-batch={},max-request-max-bytes={}",
            info.max_request_batch, info.max_request_max_bytes
        ),
        Err(e) => format!("info-failed:{}", label(&e)),
    };

    let fetched = handle
        .fetch_with_limits(batch, max_bytes, PULL_TIMEOUT_MS)
        .await;
    match fetched {
        Ok(batch) => {
            let stop = match batch.stop {
                jetstream::FetchStop::BatchFilled => "batch-filled",
                jetstream::FetchStop::Drained => "drained",
                jetstream::FetchStop::ByteLimit => "byte-limit",
            };
            for message in &batch.messages {
                let _ = message.ack().await;
            }
            format!("pull:ok:{}:{stop}:{limits}", batch.messages.len())
        }
        Err(e) => format!("pull:err:{}:{limits}", label(&e)),
    }
}

impl CoreGuest for Component {
    /// Echoes onto `reply-to` when there is one — a request/reply round trip
    /// driven entirely from inside an async export.
    async fn handle_message(msg: NatsMessage) -> Result<(), String> {
        monotonic_clock::wait_for(YIELD_NANOS).await;
        let body = String::from_utf8_lossy(&msg.body).to_string();
        if let Some(spec) = body.strip_prefix(PULL_MARKER) {
            let outcome = pull_probe(spec).await;
            return js_publish("test.results", outcome)
                .await
                .map_err(|e| label(&e));
        }
        if body.starts_with(FAIL_MARKER) {
            return Err(format!("core handler failed on `{body}`"));
        }
        if let Some(run) = body.strip_prefix(WARM_MARKER) {
            let seen = WARM_DELIVERIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            return core::publish(message("test.results", format!("warm:{run}:{seen}")))
                .await
                .map_err(|e| label(&e));
        }
        let subject = msg.reply_to.unwrap_or_else(|| "test.results".to_string());
        core::publish(message(&subject, format!("echo:{body}")))
            .await
            .map_err(|e| label(&e))
    }
}

export!(Component);
