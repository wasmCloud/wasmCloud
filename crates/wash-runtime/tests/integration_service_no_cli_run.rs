//! A p3 service does not need a `wasi:cli/run` export.
//!
//! `wasi:cli/run` is what a service uses to drive long-running work of its own
//! — an accept loop, a poller, a connection pool. It is not what makes the
//! service instance live: the host instantiates the service once and serves
//! each host-invoked export as a concurrent task on that instance (see
//! [`wash_runtime::host::trigger_service`]), and `cli/run` is only co-driven
//! alongside when it is present.
//!
//! The `svc-no-run` fixture exports `wasi:http/handler@0.3` and nothing else.
//! It reports a process-global call count, which distinguishes the two possible
//! dispatches: a single long-lived service instance makes the count climb
//! across requests, while a per-request component instance would report `1`
//! every time. `svc-counter` is the same fixture *with* a `cli/run` tick loop,
//! so the pair isolates exactly what `cli/run` buys.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::HashMap, time::Duration};

use anyhow::{Context, Result};
use tokio::time::timeout;

use wash_runtime::host::HostApi;
use wash_runtime::types::{LocalResources, Service, Workload, WorkloadStartRequest};

mod common;
use common::{http_only_host_interfaces, json_u64_field, start_host_with_p3_http_handler};

const SVC_NO_RUN_WASM: &[u8] = include_bytes!("wasm/svc_no_run.wasm");

fn svc_no_run_request(host: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: host.to_string(),
            annotations: HashMap::new(),
            service: Some(Service {
                digest: None,
                bytes: bytes::Bytes::from_static(SVC_NO_RUN_WASM),
                local_resources: LocalResources::default(),
                max_restarts: 0,
                ports: Vec::new(),
            }),
            components: vec![],
            host_interfaces: http_only_host_interfaces(host),
            volumes: vec![],
        },
    }
}

/// A service whose only export is the async `http/handler` starts, serves, and
/// keeps one instance alive across requests — no `cli/run` anywhere.
#[tokio::test]
async fn service_without_cli_run_serves_http_on_one_instance() -> Result<()> {
    let (addr, host) = start_host_with_p3_http_handler("127.0.0.1:0").await?;

    host.workload_start(svc_no_run_request("svc-no-run"))
        .await
        .context("a service exporting only wasi:http/handler should start")?;

    // No connection pooling: a GET retried on a stale pooled connection would
    // land twice on the instance and break the exactly-once call counts.
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()?;

    let mut counts = Vec::new();
    for i in 0..5 {
        let resp = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}/"))
                .header("HOST", "svc-no-run")
                .send(),
        )
        .await
        .with_context(|| format!("request {i} timed out"))?
        .with_context(|| format!("request {i} failed"))?;
        anyhow::ensure!(
            resp.status().is_success(),
            "request {i} returned {}",
            resp.status()
        );
        counts.push(json_u64_field(&resp.text().await?, "http_calls"));
    }

    assert_eq!(
        counts,
        vec![1, 2, 3, 4, 5],
        "every request must land on the same long-lived service instance; \
         a repeated 1 would mean the host fell back to instantiating per request"
    );
    Ok(())
}
