//! Minimal bare host for benchmarking: HTTP trigger only, zero plugins.
//!
//! Serves a single HTTP component workload through the same code path
//! `wash host` uses (Engine → HostBuilder → HttpServer → workload_start),
//! without NATS, washlet, or any capability plugins.
//!
//! Usage: bench_host <component.wasm> [addr]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DevRouter, HttpServer};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::types::{Component, LocalResources, Workload, WorkloadStartRequest};
use wash_runtime::wit::WitInterface;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let wasm_path = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("usage: bench_host <component.wasm> [addr]"))?;
    let addr = args.next().unwrap_or_else(|| "127.0.0.1:8090".to_string());
    let bytes = std::fs::read(&wasm_path)?;

    let engine = Engine::builder().with_pooling_allocator(true).build()?;
    let http = HttpServer::new(DevRouter::default(), addr.parse()?).await?;
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http))
        .build()?;
    let host = host.start().await?;

    let res = host
        .workload_start(WorkloadStartRequest {
            workload_id: uuid::Uuid::new_v4().to_string(),
            workload: Workload {
                namespace: "bench".to_string(),
                name: "hello".to_string(),
                annotations: HashMap::new(),
                service: None,
                components: vec![Component {
                    name: "hello".to_string(),
                    bytes: bytes.into(),
                    digest: None,
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 0,
                }],
                host_interfaces: vec![WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: HashSet::from(["incoming-handler".to_string()]),
                    version: None,
                    config: HashMap::new(),
                    name: None,
                }],
                volumes: vec![],
            },
        })
        .await?;

    eprintln!("workload: {res:?}");
    eprintln!("bench host serving on http://{addr}");
    tokio::signal::ctrl_c().await?;
    Ok(())
}
