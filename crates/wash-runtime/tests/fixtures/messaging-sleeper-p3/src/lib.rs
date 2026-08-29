//! A `wasmcloud:messaging@0.3.0` handler that awaits instead of computing, with
//! the cost split the way a real I/O-bound guest splits it.
//!
//! The messaging counterpart of the `http-sleeper` fixture, and the same
//! reasoning applies. `SETUP_NANOS` is paid **once per instance**, standing in
//! for what a guest builds on first use and then keeps: a connection pool, an
//! authenticated session, a lazily built runtime. The per-delivery sleep — the
//! milliseconds the message body names — stands in for the work itself. That
//! split is what makes instance reuse measurable: a delivery the warm set
//! cannot take is served from a store of its own and sleeps in parallel anyway,
//! so wall clock says nothing, while a setup charged per instance says exactly
//! what reuse saved.
//!
//! Three counters leave the guest through `wasi:http/handler`:
//!
//!   - `msg_peak`: the most deliveries this instance ever had in flight at
//!     once. An instance serving one at a time reports 1 however hard it is
//!     driven, so a value above 1 can only come from `maxConcurrency`.
//!   - `msg_served`: how many deliveries this instance has handled. A fresh
//!     instance starts over at zero, so a value that climbs is reuse.
//!   - `served`: how many HTTP probes it has answered. A probe is a call on the
//!     same pool and spends the same `maxInvocations` budget, so a count
//!     restarting at one is a replacement instance.
//!
//! The HTTP handler neither sleeps nor touches the messaging counters, so
//! reading them cannot perturb what it is reading.

mod bindings;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::exports::wasmcloud::messaging::handler::Guest as MsgGuest;
use bindings::wasi::clocks::monotonic_clock;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};
use bindings::wasmcloud::messaging::types::{BrokerMessage, HandleMessageError};

/// Paid once per instance, on its first delivery: what a guest sets up and
/// keeps.
const SETUP_NANOS: u64 = 100_000_000; // 100ms
/// What a delivery sleeps when its body names no duration.
const DEFAULT_DELIVERY_NANOS: u64 = 5_000_000; // 5ms

static SETUP_DONE: AtomicBool = AtomicBool::new(false);
static MSG_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
static MSG_PEAK_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
static MSG_SERVED: AtomicU64 = AtomicU64::new(0);
/// HTTP probes this instance has answered, this one included.
static SERVED: AtomicU64 = AtomicU64::new(0);

struct Component;

impl MsgGuest for Component {
    async fn handle_message(msg: BrokerMessage) -> Result<(), HandleMessageError> {
        // Drain the body first: it names how long to sleep, and a reader left
        // undrained holds the host's end of the stream open.
        let body = msg.body.collect().await;
        let sleep_nanos = match std::str::from_utf8(&body)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
        {
            Some(millis) => millis.saturating_mul(1_000_000),
            None => DEFAULT_DELIVERY_NANOS,
        };

        MSG_SERVED.fetch_add(1, Ordering::SeqCst);
        let now = MSG_IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        MSG_PEAK_IN_FLIGHT.fetch_max(now, Ordering::SeqCst);

        // First delivery on this instance pays to set up what the instance then
        // keeps. A reused instance skips it; a fresh one never can.
        if !SETUP_DONE.swap(true, Ordering::SeqCst) {
            monotonic_clock::wait_for(SETUP_NANOS).await;
        }

        // Yield to the instance's executor. Anything else spawned on this
        // instance runs while this delivery is parked here — which is the whole
        // point of the exercise.
        monotonic_clock::wait_for(sleep_nanos).await;

        MSG_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }
}

impl HttpGuest for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let served = SERVED.fetch_add(1, Ordering::SeqCst) + 1;
        let msg_peak = MSG_PEAK_IN_FLIGHT.load(Ordering::SeqCst);
        let msg_served = MSG_SERVED.load(Ordering::SeqCst);
        let body = format!(
            "{{\"msg_peak\":{msg_peak},\"msg_served\":{msg_served},\"served\":{served}}}"
        );
        Ok(make_response(200, body.into_bytes()))
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

bindings::export!(Component with_types_in bindings);
