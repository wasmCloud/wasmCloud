//! A p3 HTTP handler that grows its own linear memory and reports what it got.
//!
//! `GET /grow?mib=N` grows the memory by N MiB and answers
//! `{"granted_mib":G,"refused":B}`. The growth goes through
//! `core::arch::wasm32::memory_grow` rather than through an allocation, so a
//! refusal is *observable*: the intrinsic hands back `usize::MAX` where a Rust
//! allocation would call `handle_alloc_error` and abort, turning the host's
//! deliberate `-1` into a trap and hiding the very thing under test.
//!
//! Each page grown is written to, so the pages the host is charged for are
//! pages the kernel really backs — otherwise a lazily committing allocator
//! makes the fixture prove nothing about residency.
//!
//! The memory is never given back: a wasm linear memory cannot shrink. That is
//! the point — what releases it is the store being dropped.

mod bindings {
    wit_bindgen::generate!({
        generate_all,
    });
}

use bindings::exports::wasi::http::handler::Guest as Handler;
use bindings::wasi::http::types::{ErrorCode, Fields, Request, Response};

/// The WebAssembly page size.
const PAGE: usize = 64 * 1024;
const PAGES_PER_MIB: usize = (1024 * 1024) / PAGE;

/// Held across the grow loop so the reply can be built after the host has
/// stopped handing out memory. Several pages, so it survives however the
/// allocator chooses to carve it up.
const RESERVE_BYTES: usize = 8 * PAGE;

struct Component;

impl Handler for Component {
    async fn handle(request: Request) -> Result<Response, ErrorCode> {
        let path = request.get_path_with_query().unwrap_or_default();
        let mib = query_usize(&path, "mib=").unwrap_or(1);

        // Bought before the loop and handed back after it, so building the
        // reply is served from the allocator's free list rather than from a
        // fresh `memory.grow`.
        //
        // This is load-bearing: the loop runs until the host refuses, leaving
        // the budget with under a page to spare, and a Rust guest whose
        // allocation fails calls `handle_alloc_error` and aborts. Without the
        // reserve, the graceful -1 under test would surface as a trap — the
        // one outcome the test asserts against.
        //
        // Written to and passed through `black_box`, because a buffer that is
        // only allocated and dropped is an allocation the optimiser is free to
        // delete outright, and this fixture is built `--release`.
        let mut reserve: Vec<u8> = vec![0; RESERVE_BYTES];
        std::hint::black_box(&mut reserve);

        let mut granted_pages = 0;
        let mut refused = false;
        // One page at a time so the reply can say exactly where the budget cut
        // in, rather than only that the whole request failed.
        for _ in 0..(mib * PAGES_PER_MIB) {
            match grow_one_page() {
                Some(base) => {
                    touch(base);
                    granted_pages += 1;
                }
                None => {
                    refused = true;
                    break;
                }
            }
        }

        // Everything below here allocates — the body, the header fields, the
        // stream and future pair, the spawned task. This is what pays for it.
        std::hint::black_box(&reserve);
        drop(reserve);

        let body = format!(
            "{{\"granted_mib\":{},\"refused\":{refused}}}",
            granted_pages / PAGES_PER_MIB
        );
        Ok(make_response(body.into_bytes()))
    }
}

/// Grow the default memory by one page, returning the byte offset of the new
/// page, or `None` when the host refused.
#[cfg(target_arch = "wasm32")]
fn grow_one_page() -> Option<usize> {
    let previous_pages = core::arch::wasm32::memory_grow::<0>(1);
    // `memory.grow` answers `-1` on failure, which the intrinsic surfaces as
    // `usize::MAX`. Every other value is the page count from before the grow.
    (previous_pages != usize::MAX).then(|| previous_pages * PAGE)
}

#[cfg(not(target_arch = "wasm32"))]
fn grow_one_page() -> Option<usize> {
    unreachable!("this fixture only builds for wasm32")
}

/// Write to the page at `base` so the host is charged for memory something
/// actually resides in.
fn touch(base: usize) {
    // Safety: `base` is the first byte of a page `memory.grow` just returned,
    // so the whole page is addressable and owned by nothing else.
    unsafe {
        core::ptr::write_volatile(base as *mut u8, 0xAB);
        core::ptr::write_volatile((base + PAGE - 1) as *mut u8, 0xAB);
    }
}

/// The digits following `key` in `path`, if it carries that key.
fn query_usize(path: &str, key: &str) -> Option<usize> {
    let (_, rest) = path.split_once(key)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn make_response(body: Vec<u8>) -> Response {
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
    let _ = response.set_status_code(200);
    response
}

bindings::export!(Component with_types_in bindings);
