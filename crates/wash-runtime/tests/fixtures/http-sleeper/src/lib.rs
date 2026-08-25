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
//!
//! `/trap` traps instead of replying, so a test can poison an instance's store
//! on purpose and check what that costs: the calls sharing that instance, and
//! nothing else. `/wedge` parks for an hour before producing the response
//! head — a guest wedged awaiting I/O that will never arrive — so a test can
//! check what the host's per-call timeout does about it.
//!
//! Each reply also carries `served`, this instance's own request count. That
//! is how a test tells a *retired* instance from a merely recovered slot: a
//! replacement instance starts its count over at one.

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
/// Requests this instance has begun serving, `/wedge` included.
static SERVED: AtomicU64 = AtomicU64::new(0);

struct Component;

impl HttpGuest for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request.get_path_with_query().unwrap_or_default();
        let trap = path.starts_with("/trap");

        let served = SERVED.fetch_add(1, Ordering::SeqCst) + 1;

        // Wedged: the response head never comes. What bounds the caller's wait
        // (and this instance's fate) is the host's per-call timeout alone.
        if path.starts_with("/wedge") {
            monotonic_clock::wait_for(3_600_000_000_000).await; // one hour
        }

        // `/sse?frames=N&gap_ms=M` answers at once, then emits N frames M ms
        // apart. A client disconnecting mid-stream is not noticed until the
        // next write, so with a long gap the response is abandoned while the
        // guest is asleep — healthy, and its store must be left alone.
        if path.starts_with("/sse") {
            let frames = query_u64(&path, "frames=").unwrap_or(10);
            let gap_ms = query_u64(&path, "gap_ms=").unwrap_or(1_000);
            return Ok(make_sse_response(frames, gap_ms));
        }

        // Slow but healthy: `/slow?ms=N` sleeps N ms and then answers. It
        // yields the whole time, however large N is.
        if path.starts_with("/slow") {
            let ms = query_u64(&path, "ms=").unwrap_or(1_000);
            monotonic_clock::wait_for(ms.saturating_mul(1_000_000)).await;
        }

        // Chatty but healthy: `/chatter?hops=N&hop_ms=M` computes briefly,
        // awaits M ms, and repeats N times before answering — a guest that
        // yields constantly but never pauses for long. Each wake lands on an
        // expired epoch deadline, so this is the wake pattern that a sampled
        // view cannot tell from a pinned guest within a single window.
        if path.starts_with("/chatter") {
            let hops = query_u64(&path, "hops=").unwrap_or(4);
            let hop_ms = query_u64(&path, "hop_ms=").unwrap_or(200);
            for _ in 0..hops {
                monotonic_clock::wait_for(hop_ms.saturating_mul(1_000_000)).await;
            }
        }

        // Spinning: never yields, so it is unreachable by every host-side
        // timeout — those are futures, and this call's poll never returns for
        // one to be polled. Only the epoch deadline compiled into this loop's
        // back-edge can end it. `black_box` keeps the loop from being optimised
        // away; the counter is never read.
        //
        // `/spin` runs forever. `/spin?ms=N` runs for N milliseconds and then
        // replies normally, which is how a test tells "trapped" from "measured
        // and left alone" — the bounded spin exceeds the budget either way, but
        // only a store that is never trapped lives to answer.
        //
        // `monotonic_clock::now()` is a plain host call, not an await: it
        // returns on the same fiber without suspending the guest, so this loop
        // still never yields to the executor.
        if path.starts_with("/spin") {
            // `0` means "forever": no bound was asked for.
            let deadline = spin_deadline_nanos(&path);
            let mut n: u64 = 0;
            loop {
                n = std::hint::black_box(n.wrapping_add(1));
                if deadline != 0 && monotonic_clock::now() >= deadline {
                    break;
                }
            }
        }

        // The realistic shape of the same failure: a component whose *input* —
        // not its code — drives it into an unbounded loop. `/redos?n=N` matches
        // a crafted string against `^(a+)+$` with the naive backtracking a JS or
        // PCRE engine uses, which is exponential in `n`. No host calls, so the
        // guest never yields, exactly as with `/spin` — but here nothing is
        // deliberately looping and nothing looks wrong in review.
        if path.starts_with("/redos") {
            // 2^n backtracks. The default is far past "slow" and into "never
            // finishes": ~10^15 steps, months of CPU.
            let n = query_u64(&path, "n=").unwrap_or(50) as usize;
            // `n` 'a's then a byte the pattern cannot match, so every one of the
            // 2^(n-1) ways to split the run is tried before the match fails.
            let mut subject = vec![b'a'; n];
            subject.push(b'!');
            let matched = redos_match(&subject, 0);
            return Ok(make_response(
                200,
                format!("{{\"matched\":{matched}}}").into_bytes(),
            ));
        }

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

        // Trap after the yield, so anything sharing this instance is already
        // in flight and goes down with the store.
        assert!(!trap, "requested trap");

        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        let peak = PEAK_IN_FLIGHT.load(Ordering::SeqCst);
        let body = format!("{{\"peak_in_flight\":{peak},\"served\":{served}}}");
        Ok(make_response(200, body.into_bytes()))
    }
}

/// The monotonic instant `/spin?ms=N` should stop at, or `0` for "run forever"
/// when the path carries no `ms=`.
fn spin_deadline_nanos(path: &str) -> u64 {
    match query_u64(path, "ms=") {
        Some(ms) => monotonic_clock::now().saturating_add(ms.saturating_mul(1_000_000)),
        None => 0,
    }
}

/// The digits following `key` in `path`, if it carries that key.
fn query_u64(path: &str, key: &str) -> Option<u64> {
    let (_, rest) = path.split_once(key)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// `^(a+)+$` matched against `subject` from `at`, the way a backtracking engine
/// does it: the inner `a+` takes one or more, the outer `+` repeats, and every
/// split is retried on failure. Exponential when the subject ends in a byte the
/// pattern cannot match — the classic ReDoS.
///
/// Recursion depth is bounded by the subject length; it is the *breadth* that
/// explodes, so this cannot overflow the stack before the epoch deadline sees
/// it.
fn redos_match(subject: &[u8], at: usize) -> bool {
    if at == subject.len() {
        return true; // matched `$`
    }
    let mut end = at;
    while end < subject.len() && subject[end] == b'a' {
        end += 1;
        if redos_match(subject, end) {
            return true;
        }
    }
    false
}

/// A `text/event-stream` response whose frames are produced `gap_ms` apart,
/// awaiting the clock in between.
fn make_sse_response(frames: u64, gap_ms: u64) -> Response {
    let headers = Fields::new();
    let _ = headers.set(&"content-type".to_string(), &[b"text/event-stream".to_vec()]);
    let (mut tx, rx) = bindings::wit_stream::new();
    let (trailers_tx, trailers_rx) = bindings::wit_future::new(|| Ok(None));
    wit_bindgen::spawn_local(async move {
        for n in 0..frames {
            monotonic_clock::wait_for(gap_ms.saturating_mul(1_000_000)).await;
            tx.write_all(format!("data: {n}\n\n").into_bytes()).await;
        }
        drop(tx);
        let _ = trailers_tx.write(Ok(None)).await;
    });
    let (response, _result) = Response::new(headers, Some(rx), trailers_rx);
    let _ = response.set_status_code(200);
    response
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
