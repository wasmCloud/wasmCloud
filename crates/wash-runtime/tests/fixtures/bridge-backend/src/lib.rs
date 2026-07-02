//! Stateless backend exporting `wasmcloud:bridge/ops`. A fresh instance handles
//! each call (instantiated in its own ephemeral store by the host bridge), so
//! the process-global `BUMPS` counter never survives past a single call —
//! `bump()` always returns 1.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "backend", generate_all });
}

use std::sync::atomic::{AtomicU64, Ordering};

use bindings::exports::wasmcloud::bridge::ops::{Chunked, Guest};
use wit_bindgen::{FutureReader, StreamReader, StreamResult};

static BUMPS: AtomicU64 = AtomicU64::new(0);

struct Component;

impl Guest for Component {
    async fn add(a: u64, b: u64) -> u64 {
        a + b
    }

    async fn bump() -> u64 {
        BUMPS.fetch_add(1, Ordering::SeqCst) + 1
    }

    async fn block(iterations: u64) -> u64 {
        // Tight CPU spin with no `.await` — monopolizes this instance's store
        // executor for the duration. `black_box` keeps the loop from being
        // optimized away.
        let mut acc: u64 = 0;
        let mut i: u64 = 0;
        while i < iterations {
            acc = std::hint::black_box(acc.wrapping_mul(31).wrapping_add(i));
            i += 1;
        }
        acc
    }

    async fn consume(mut data: StreamReader<u8>) -> u64 {
        let mut total: u64 = 0;
        loop {
            let (result, chunk) = data.read(Vec::with_capacity(4096)).await;
            total += chunk.len() as u64;
            if matches!(result, StreamResult::Dropped) {
                break;
            }
        }
        total
    }

    async fn produce(count: u64) -> StreamReader<u8> {
        let (mut tx, rx) = bindings::wit_stream::new();
        wit_bindgen::spawn_local(async move {
            let chunk = vec![b'x'; 256];
            let mut written: u64 = 0;
            while written < count {
                let n = ((count - written) as usize).min(chunk.len());
                tx.write_all(chunk[..n].to_vec()).await;
                written += n as u64;
            }
            drop(tx);
        });
        rx
    }

    async fn sum(mut data: StreamReader<u32>) -> u64 {
        let mut total: u64 = 0;
        loop {
            let (result, chunk) = data.read(Vec::with_capacity(1024)).await;
            for v in &chunk {
                total += *v as u64;
            }
            if matches!(result, StreamResult::Dropped) {
                break;
            }
        }
        total
    }

    async fn delayed(value: u64) -> FutureReader<u64> {
        let (tx, rx) = bindings::wit_future::new(|| 0u64);
        wit_bindgen::spawn_local(async move {
            let _ = tx.write(value).await;
        });
        rx
    }

    async fn relay(m: Chunked) -> u64 {
        match m {
            Chunked::None => 0,
            Chunked::Data(mut s) => {
                let mut total: u64 = 0;
                loop {
                    let (result, chunk) = s.read(Vec::with_capacity(4096)).await;
                    total += chunk.len() as u64;
                    if matches!(result, StreamResult::Dropped) {
                        break;
                    }
                }
                total
            }
        }
    }
}

mod export {
    #![allow(unsafe_code)]
    use super::{bindings, Component};
    bindings::export!(Component with_types_in bindings);
}
