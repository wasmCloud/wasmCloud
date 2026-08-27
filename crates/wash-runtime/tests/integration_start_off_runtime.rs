//! Pins that starting a workload leaves the async runtime free to make
//! progress.
//!
//! Compiling a component is synchronous, CPU-bound Cranelift work. Run on a
//! runtime thread it holds that thread for the whole compile, and the host's
//! NATS connection task is one of the things that then cannot be polled — a
//! host that stops draining its socket is dropped by the server as a slow
//! consumer, which the operator sees as a host that has gone away.
//!
//! This is why the check is not an end-to-end one: from outside, a host that
//! compiled on its runtime thread and one that did not both just start the
//! workload. The difference is which thread was busy, and the way to make that
//! observable is to give the runtime exactly one worker.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use wash_runtime::host::{Host, HostApi};
use wash_runtime::types::{Component, Workload, WorkloadStartRequest};

/// Big enough that compiling it is unmistakably longer than the scheduling
/// noise around it.
const COMPONENT_WASM: &[u8] = include_bytes!("wasm/http_egress_pool.wasm");

/// `#[tokio::test]` runs on a current-thread runtime: one worker. Synchronous
/// work on it stops everything else, so "the compile blocked the runtime" and
/// "it did not" are different in kind rather than in degree. On a multi-threaded
/// runtime, occupying one worker out of many hides until the machine is loaded
/// — which is exactly how this reached production.
#[tokio::test]
async fn compiling_a_workload_leaves_the_runtime_free() -> Result<()> {
    let host = Host::builder().build()?;

    // Ticks whenever the runtime has a moment to poll it.
    let ticks = Arc::new(AtomicUsize::new(0));
    let ticker = tokio::spawn({
        let ticks = Arc::clone(&ticks);
        async move {
            loop {
                ticks.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
            }
        }
    });
    tokio::task::yield_now().await;
    let before = ticks.load(Ordering::SeqCst);

    let response = host
        .workload_start(WorkloadStartRequest {
            workload_id: "compile-off-runtime".to_string(),
            workload: Workload {
                namespace: "wasmcloud".to_string(),
                name: "compile-off-runtime".to_string(),
                annotations: Default::default(),
                service: None,
                components: vec![Component {
                    name: "compiled".to_string(),
                    bytes: COMPONENT_WASM.into(),
                    digest: None,
                    local_resources: Default::default(),
                    pool_size: 0,
                    max_invocations: 0,
                    max_concurrency: 0,
                }],
                host_interfaces: vec![],
                volumes: vec![],
            },
        })
        .await?;

    let during = ticks.load(Ordering::SeqCst) - before;
    ticker.abort();

    // Whether the workload itself came up is beside the point — it may want
    // plugins this bare host has not got. The compile ran either way, and that
    // is what had to leave the runtime thread.
    let _ = response;

    assert!(
        during > 100,
        "the runtime polled another task only {during} times while a component compiled; \
         a compile on the runtime thread starves the host's NATS connection task with it"
    );
    Ok(())
}
