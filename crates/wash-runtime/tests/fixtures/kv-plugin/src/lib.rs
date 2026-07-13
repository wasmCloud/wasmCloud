//! Host component plugin exporting the bespoke, handle-free `acme:kv/store`
//! capability, backed by a process-global in-memory map.
//!
//! Unlike the ephemeral `bridge-backend` (a fresh instance per call), this
//! component is instantiated ONCE into a long-lived, host-scoped store and
//! serves every workload that imports `acme:kv/store`. Because the instance
//! persists, its `STORE` survives across calls — the property the host
//! component singleton exists to provide.

mod bindings {
    #![allow(unsafe_code)]
    wit_bindgen::generate!({ world: "kv-plugin", generate_all });
}

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use bindings::exports::acme::kv::store::{Bucket, Guest, GuestBucket};
use bindings::wasi::clocks::monotonic_clock;
use bindings::wasmcloud::host::cancel;
use wit_bindgen::{FutureReader, StreamReader, StreamResult};

/// Number of `bucket` resources dropped, incremented by `Bucket`'s destructor —
/// lets a test prove a caller's proxy drop frees the real resource here.
static DROPPED_BUCKETS: AtomicU64 = AtomicU64::new(0);

/// A guest-owned key-value partition behind the exported `bucket` resource.
struct BucketState {
    data: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl Drop for BucketState {
    fn drop(&mut self) {
        DROPPED_BUCKETS.fetch_add(1, Ordering::SeqCst);
    }
}

impl GuestBucket for BucketState {
    async fn get(&self, key: String) -> Option<Vec<u8>> {
        self.data.lock().unwrap().get(&key).cloned()
    }

    async fn set(&self, key: String, value: Vec<u8>) {
        self.data.lock().unwrap().insert(key, value);
    }
}

/// Persistent store state. Held only within synchronous blocks (never across an
/// `.await`), so a plain `Mutex` is sufficient even though concurrent capability
/// calls interleave cooperatively on this one instance.
static STORE: Mutex<BTreeMap<String, Vec<u8>>> = Mutex::new(BTreeMap::new());

/// Per-caller partitions: caller workload id -> that caller's own map. Proves
/// per-caller state isolation on the shared singleton.
static PARTITIONS: Mutex<BTreeMap<String, BTreeMap<String, Vec<u8>>>> =
    Mutex::new(BTreeMap::new());

/// `begin` name -> the host job id it registered, so `cancel-job` can look it up.
static BEGUN: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

/// `begin` name -> ticks completed, so a test can observe how far a cancelled
/// `begin` got without depending on the (aborted) caller's own response.
static PROGRESS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

struct Component;

impl Guest for Component {
    async fn set(key: String, value: Vec<u8>) {
        STORE.lock().unwrap().insert(key, value);
    }

    async fn get(key: String) -> Option<Vec<u8>> {
        STORE.lock().unwrap().get(&key).cloned()
    }

    async fn delete(key: String) {
        STORE.lock().unwrap().remove(&key);
    }

    async fn pset(key: String, value: Vec<u8>) {
        // Partition by the calling workload; `get-workload-id` is a sync host
        // import that is exact under concurrency (resolved from this call's task).
        let caller = bindings::wasmcloud::host::identity::get_workload_id();
        PARTITIONS
            .lock()
            .unwrap()
            .entry(caller)
            .or_default()
            .insert(key, value);
    }

    async fn pget(key: String) -> Option<Vec<u8>> {
        let caller = bindings::wasmcloud::host::identity::get_workload_id();
        PARTITIONS
            .lock()
            .unwrap()
            .get(&caller)
            .and_then(|partition| partition.get(&key).cloned())
    }

    async fn slow(millis: u64) -> u64 {
        // Await a timer so this task YIELDS the store's cooperative executor.
        // A concurrent fast call spawned as its own task can therefore run to
        // completion before this one returns.
        monotonic_clock::wait_for(millis.saturating_mul(1_000_000)).await;
        millis
    }

    async fn total(mut data: StreamReader<u8>) -> u64 {
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

    async fn emit(count: u64) -> StreamReader<u8> {
        let (mut tx, rx) = bindings::wit_stream::new();
        wit_bindgen::spawn_local(async move {
            let chunk = vec![b'k'; 256];
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

    async fn eventually(value: u64) -> FutureReader<u64> {
        let (tx, rx) = bindings::wit_future::new(|| 0u64);
        wit_bindgen::spawn_local(async move {
            let _ = tx.write(value).await;
        });
        rx
    }

    async fn recurse(n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            // Call our OWN capability through the self-import, re-entering the
            // plugin store across the bridge. Each hop deepens the call chain;
            // a large `n` trips the host's depth guard, which traps this call.
            1 + bindings::acme::kv::store::recurse(n - 1).await
        }
    }

    async fn boom() {
        panic!("kv-plugin boom: deliberate guest trap for the poisoning negative test");
    }

    type Bucket = BucketState;

    async fn open(_name: String) -> Bucket {
        Bucket::new(BucketState {
            data: Mutex::new(BTreeMap::new()),
        })
    }

    async fn dropped_buckets() -> u64 {
        DROPPED_BUCKETS.load(Ordering::SeqCst)
    }

    async fn begin(name: String, ticks: u64, tick_ms: u64) -> u64 {
        // Register this invocation's host job so another caller can cancel it by
        // name, then run a long loop that cooperatively checks for cancellation
        // each tick and returns early if asked. `progress`/the return value report
        // how far it got — the full `ticks` if it ran to completion, fewer if
        // cancelled.
        let job = cancel::current_job();
        BEGUN.lock().unwrap().insert(name.clone(), job);
        PROGRESS.lock().unwrap().insert(name.clone(), 0);
        let mut done = 0u64;
        while done < ticks {
            monotonic_clock::wait_for(tick_ms.saturating_mul(1_000_000)).await;
            if cancel::is_cancelled() {
                break;
            }
            done += 1;
            PROGRESS.lock().unwrap().insert(name.clone(), done);
        }
        done
    }

    async fn cancel_job(name: String) -> bool {
        let job = BEGUN.lock().unwrap().get(&name).copied();
        match job {
            Some(job) => cancel::request_cancel(job),
            None => false,
        }
    }

    async fn progress(name: String) -> u64 {
        PROGRESS.lock().unwrap().get(&name).copied().unwrap_or(0)
    }
}

mod export {
    #![allow(unsafe_code)]
    use super::{bindings, Component};
    bindings::export!(Component with_types_in bindings);
}
