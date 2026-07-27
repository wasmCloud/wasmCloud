//! A p3 HTTP handler that awaits instead of computing, with the cost split the
//! way a real I/O-bound guest splits it.
//!
//! `SETUP_NANOS` is paid **once per instance**, standing in for what a guest
//! builds on first use and then keeps: a connection pool, an authenticated
//! session, a lazily built runtime. `PER_CALL_NANOS` is paid by every call,
//! standing in for the query itself.
//!
//! That split is what makes instance reuse measurable. A fixture that slept the
//! same amount on every call could not show it: a request that misses the warm
//! set is served from a store of its own and sleeps *in parallel* anyway, so
//! the only thing reuse saves there is the store itself — microseconds against
//! a sleep. Charge the setup per instance instead and reuse shows its actual
//! worth, which is not paying that cost per request.
//!
//! Each reply carries the peak number of calls this instance had in flight at
//! once. That is the timing-independent signal: an instance serving one call at
//! a time reports `1` however hard it is driven.

mod bindings;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use bindings::exports::wasi::http::handler::Guest as HttpGuest;
use bindings::wasi::clocks::monotonic_clock;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

/// Paid once per instance, on its first call: what a guest sets up and keeps.
const SETUP_NANOS: u64 = 100_000_000; // 100ms
/// Paid by every call: the work itself.
const PER_CALL_NANOS: u64 = 5_000_000; // 5ms

static SETUP_DONE: AtomicBool = AtomicBool::new(false);
static IN_FLIGHT: AtomicU64 = AtomicU64::new(0);
static PEAK_IN_FLIGHT: AtomicU64 = AtomicU64::new(0);

struct Component;

impl HttpGuest for Component {
    async fn handle(_request: Request) -> Result<Response, ErrorCode> {
        let now = IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
        PEAK_IN_FLIGHT.fetch_max(now, Ordering::SeqCst);

        // First call on this instance pays to set up what the instance then
        // keeps. A reused instance skips it; a fresh one never can.
        if !SETUP_DONE.swap(true, Ordering::SeqCst) {
            monotonic_clock::wait_for(SETUP_NANOS).await;
        }

        // Yield to the instance's executor. Anything else spawned on this
        // instance runs while this call is parked here — which is the whole
        // point of the exercise.
        monotonic_clock::wait_for(PER_CALL_NANOS).await;

        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        let peak = PEAK_IN_FLIGHT.load(Ordering::SeqCst);
        let body = format!("{{\"peak_in_flight\":{peak}}}");
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
