//! Minimal bare host for benchmarking: HTTP trigger only, zero plugins.
//!
//! Serves a workload through the same code path `wash host` uses
//! (Engine → HostBuilder → HttpServer → workload_start), without NATS,
//! washlet, or any capability plugins.
//!
//! Usage: bench_host [--addr host:port] [--router dev|dynamic] [--host NAME]
//!                   [--replicas N] [--service svc.wasm] [component.wasm ...]
//!
//! Examples:
//!   bench_host hello.wasm                        # one per-request component
//!   bench_host --service svc_counter.wasm        # long-lived p3 trigger service
//!   bench_host caller.wasm callee.wasm           # linked c2c workload
//!   bench_host --router dynamic --host bench --replicas 4 --service svc.wasm
//!       # 4 replica workloads behind one hostname (drive with `oha -H "host: bench"`)

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DevRouter, DynamicRouter, HttpServer};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::types::{Component, LocalResources, Service, Workload, WorkloadStartRequest};
use wash_runtime::wit::WitInterface;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error")),
        )
        .init();
    let mut addr = "127.0.0.1:8090".to_string();
    let mut service_path: Option<String> = None;
    let mut component_paths: Vec<String> = Vec::new();
    let mut router = "dev".to_string();
    let mut hostname = "bench".to_string();
    let mut replicas: u32 = 1;
    let mut pool_size: i32 = 0;
    let mut max_invocations: i32 = 0;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--addr" => addr = args.next().expect("--addr needs a value"),
            "--service" => service_path = Some(args.next().expect("--service needs a value")),
            "--router" => router = args.next().expect("--router needs a value"),
            "--host" => hostname = args.next().expect("--host needs a value"),
            "--replicas" => {
                replicas = args
                    .next()
                    .expect("--replicas needs a value")
                    .parse()
                    .expect("--replicas must be a number")
            }
            "--pool" => {
                pool_size = args
                    .next()
                    .expect("--pool needs a value")
                    .parse()
                    .expect("--pool must be a number")
            }
            "--max-inv" => {
                max_invocations = args
                    .next()
                    .expect("--max-inv needs a value")
                    .parse()
                    .expect("--max-inv must be a number")
            }
            _ => component_paths.push(a),
        }
    }
    anyhow::ensure!(
        service_path.is_some() || !component_paths.is_empty(),
        "usage: bench_host [--addr host:port] [--router dev|dynamic] [--host NAME] [--replicas N] [--service svc.wasm] [component.wasm ...]"
    );

    let engine = Engine::builder().with_pooling_allocator(true).build()?;
    let host = match router.as_str() {
        "dynamic" => {
            let http = HttpServer::new(DynamicRouter::default(), addr.parse()?).await?;
            HostBuilder::new()
                .with_engine(engine)
                .with_http_handler(Arc::new(http))
        }
        _ => {
            let http = HttpServer::new(DevRouter::default(), addr.parse()?).await?;
            HostBuilder::new()
                .with_engine(engine)
                .with_http_handler(Arc::new(http))
        }
    }
    .with_plugin(Arc::new(
        wash_runtime::plugin::wasi_logging::TracingLogger::default(),
    ))?
    .with_plugin(Arc::new(
        wash_runtime::plugin::wasi_config::DynamicConfig::default(),
    ))?
    .build()?;
    let host = host.start().await?;

    let service_bytes = match &service_path {
        Some(p) => Some(bytes::Bytes::from(std::fs::read(p)?)),
        None => None,
    };
    let component_bytes = component_paths
        .iter()
        .map(|p| {
            Ok((
                std::path::Path::new(p)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("component")
                    .to_string(),
                bytes::Bytes::from(std::fs::read(p)?),
            ))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let mut interface_config = HashMap::new();
    if router == "dynamic" {
        interface_config.insert("host".to_string(), hostname.clone());
    }

    for i in 0..replicas {
        let res = host
            .workload_start(WorkloadStartRequest {
                workload_id: uuid::Uuid::new_v4().to_string(),
                workload: Workload {
                    namespace: "bench".to_string(),
                    name: format!("bench-{i}"),
                    annotations: HashMap::new(),
                    service: service_bytes.as_ref().map(|b| Service {
                        bytes: b.clone(),
                        digest: None,
                        local_resources: LocalResources::default(),
                        max_restarts: 3,
                    }),
                    components: component_bytes
                        .iter()
                        .map(|(name, b)| Component {
                            name: name.clone(),
                            bytes: b.clone(),
                            digest: None,
                            local_resources: LocalResources::default(),
                            pool_size,
                            max_invocations,
                        })
                        .collect(),
                    host_interfaces: vec![WitInterface {
                        namespace: "wasi".to_string(),
                        package: "http".to_string(),
                        interfaces: HashSet::from([
                            "incoming-handler".to_string(),
                            "handler".to_string(),
                        ]),
                        version: None,
                        config: interface_config.clone(),
                        name: None,
                    }],
                    volumes: vec![],
                },
            })
            .await?;
        eprintln!("replica {i}: {:?}", res.workload_status.workload_state);
    }

    eprintln!("bench host serving on http://{addr} (router={router}, host={hostname}, replicas={replicas})");
    tokio::signal::ctrl_c().await?;
    Ok(())
}
