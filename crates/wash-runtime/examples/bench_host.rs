//! Minimal bare host for benchmarking: HTTP trigger only, zero plugins.
//!
//! Serves a workload through the same code path `wash host` uses
//! (Engine → HostBuilder → HttpServer → workload_start), without NATS,
//! washlet, or any capability plugins.
//!
//! Usage: bench_host [--addr host:port] [--service svc.wasm] [component.wasm ...]
//!
//! Examples:
//!   bench_host hello.wasm                        # one per-request component
//!   bench_host --service svc_counter.wasm        # long-lived p3 trigger service
//!   bench_host caller.wasm callee.wasm           # linked c2c workload

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DevRouter, HttpServer};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::types::{
    Component, LocalResources, Service, Workload, WorkloadStartRequest,
};
use wash_runtime::wit::WitInterface;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut addr = "127.0.0.1:8090".to_string();
    let mut service_path: Option<String> = None;
    let mut component_paths: Vec<String> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--addr" => addr = args.next().expect("--addr needs a value"),
            "--service" => service_path = Some(args.next().expect("--service needs a value")),
            _ => component_paths.push(a),
        }
    }
    anyhow::ensure!(
        service_path.is_some() || !component_paths.is_empty(),
        "usage: bench_host [--addr host:port] [--service svc.wasm] [component.wasm ...]"
    );

    let engine = Engine::builder().with_pooling_allocator(true).build()?;
    let http = HttpServer::new(DevRouter::default(), addr.parse()?).await?;
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http))
        .build()?;
    let host = host.start().await?;

    let service = match &service_path {
        Some(p) => Some(Service {
            bytes: std::fs::read(p)?.into(),
            digest: None,
            local_resources: LocalResources::default(),
            max_restarts: 3,
        }),
        None => None,
    };

    let components = component_paths
        .iter()
        .map(|p| {
            Ok(Component {
                name: std::path::Path::new(p)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("component")
                    .to_string(),
                bytes: std::fs::read(p)?.into(),
                digest: None,
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 0,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let res = host
        .workload_start(WorkloadStartRequest {
            workload_id: uuid::Uuid::new_v4().to_string(),
            workload: Workload {
                namespace: "bench".to_string(),
                name: "bench".to_string(),
                annotations: HashMap::new(),
                service,
                components,
                host_interfaces: vec![WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: HashSet::from([
                        "incoming-handler".to_string(),
                        "handler".to_string(),
                    ]),
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
