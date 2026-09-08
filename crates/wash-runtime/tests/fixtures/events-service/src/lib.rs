//! Workload SERVICE exporting `acme:events/handler`, which a host component
//! plugin calls back into.
//!
//! The component fixture beside this one (`events-caller`) is the same
//! direction on a component; this is the shape that has no per-call
//! instantiation to fall back on, so a plugin reaches it only if the host
//! delivers the call to the running service itself.
//!
//! Every reply is `{EVENT_TAG}:{message}:{n}`, where `n` counts the calls this
//! instance has served. The tag says WHICH workload handled it and the count
//! says it was handled on one long-lived instance rather than a fresh one.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "event-service", generate_all });
}

use std::sync::atomic::{AtomicU32, Ordering};

use bindings::exports::acme::events::bulk::Guest as BulkGuest;
use bindings::exports::acme::events::handler::{CallError, Guest as HandlerGuest};
use bindings::exports::wasi::cli::run::Guest as RunGuest;
use bindings::wasi::clocks::monotonic_clock;
use wit_bindgen::{StreamReader, StreamResult};

/// Bytes per read on the way in, and per write on the way out.
const CHUNK: usize = 256;

/// Calls served by this instance, restarting at zero in a fresh one.
static SERVED: AtomicU32 = AtomicU32::new(0);

struct Component;

/// This workload's own name, from its manifest environment, so two deploys of
/// this same wasm can be told apart in a callback's reply.
fn tag() -> String {
    std::env::var("EVENT_TAG").unwrap_or_else(|_| "untagged".to_string())
}

impl HandlerGuest for Component {
    async fn notify(message: String) -> Result<String, CallError> {
        let served = SERVED.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(format!("{}:{message}:{served}", tag()))
    }
}

impl BulkGuest for Component {
    /// Reads a stream the plugin opened in ITS store: the bytes arrive over the
    /// host's pump, on this instance, while it goes on serving everything else.
    async fn absorb(mut data: StreamReader<u8>) -> Result<u64, CallError> {
        let mut total: u64 = 0;
        loop {
            let (result, chunk) = data.read(Vec::with_capacity(CHUNK)).await;
            total += chunk.len() as u64;
            if matches!(result, StreamResult::Dropped) {
                break;
            }
        }
        Ok(total)
    }

    /// Opens a stream in this store for the plugin to drain, which keeps
    /// producing after the call itself has returned.
    async fn emit(count: u64) -> Result<StreamReader<u8>, CallError> {
        let (mut tx, rx) = bindings::wit_stream::new();
        wit_bindgen::spawn_local(async move {
            let chunk = vec![b'y'; CHUNK];
            let mut written: u64 = 0;
            while written < count {
                let n = ((count - written) as usize).min(chunk.len());
                tx.write_all(chunk[..n].to_vec()).await;
                written += n as u64;
            }
            drop(tx);
        });
        Ok(rx)
    }
}

impl RunGuest for Component {
    async fn run() -> Result<(), ()> {
        // Long-lived: park rather than returning, so the service is still the
        // workload's running item when the plugin calls into it.
        loop {
            monotonic_clock::wait_for(60_000_000_000).await;
        }
    }
}

mod export {
    #![allow(unsafe_code)]
    use super::{Component, bindings};
    bindings::export!(Component with_types_in bindings);
}
