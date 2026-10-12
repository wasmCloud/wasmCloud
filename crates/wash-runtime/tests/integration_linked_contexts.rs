//! Per-component contexts across deeper link shapes, and the component cap.
//!
//! - A -> B -> C, every hop in A's store on the shared-store path
//!   (`future`-carrying calls), each parked while the hop above keeps running:
//!   every hop must be served with its own `WHO` and `allowedHosts`.
//! - The same chain with a read-only volume on each hop: the root and the
//!   innermost callee each see only their own mount.
//! - The cap: it counts components that link to one another. 257 components
//!   that never link start; 256 that do are refused before anything starts.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::timeout;

use wasmtime_wasi_http::{RequestOptions, WasiBody};

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DynamicRouter, Ingress, OutgoingHandler},
    },
    plugin::{wasi_config::DynamicConfig, wasi_logging::TracingLogger},
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const CALLER_WASM: &[u8] = include_bytes!("wasm/ctx_race_nest_caller_p3.wasm");
const MIDDLE_WASM: &[u8] = include_bytes!("wasm/ctx_race_middle_p3.wasm");
const VOL_CALLEE_WASM: &[u8] = include_bytes!("wasm/vol_callee_p3.wasm");
const CALLEE_WASM: &[u8] = include_bytes!("wasm/ctx_race_callee_p3.wasm");
/// Imports the `gate` interface `CALLEE_WASM` exports.
const GATE_CALLER_WASM: &[u8] = include_bytes!("wasm/ctx_race_caller_p3.wasm");

/// Refuses every network send, so a permitted request reads as 502 and a
/// policy denial as 403 without any real network.
struct RefuseOutgoingHandler;

impl OutgoingHandler for RefuseOutgoingHandler {
    fn send_request(
        &self,
        _workload_id: &str,
        _request: hyper::Request<WasiBody>,
        _options: Option<RequestOptions>,
        _fut: wash_runtime::host::http::RequestIoFuture,
    ) -> wash_runtime::host::http::SendFuture {
        Box::new(async { Err(wasmtime_wasi_http::Error::ConnectionRefused) })
    }
}

async fn start_host() -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::builder(DynamicRouter::default(), "127.0.0.1:0".parse()?)
        .outgoing_handler(RefuseOutgoingHandler)
        .build()
        .await?;
    let bound_addr = ingress.addr();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(TracingLogger::default()))?
        .with_plugin(Arc::new(DynamicConfig::default()))?
        .build()?;
    let host = host.start().await.context("Failed to start host")?;
    Ok((bound_addr, host))
}

fn component(name: &str, wasm: &'static [u8], allowed_hosts: &[&str]) -> Component {
    let parsed: Vec<wash_runtime::host::allowed_hosts::AllowedHost> = allowed_hosts
        .iter()
        .map(|s| s.parse().expect("invalid allowed_hosts entry"))
        .collect();
    Component {
        name: name.to_string(),
        digest: None,
        bytes: bytes::Bytes::from_static(wasm),
        local_resources: LocalResources {
            memory_limit_mb: 128,
            cpu_limit: 1,
            config: HashMap::new(),
            environment: HashMap::from([("WHO".to_string(), name.to_string())]),
            volume_mounts: vec![],
            allowed_hosts: parsed.into(),
            allowed_ip_name_lookups: Default::default(),
            allowed_host_loopback_ports: Default::default(),
        },
        pool_size: 1,
        max_invocations: 100,
        max_concurrency: 1,
        ..Default::default()
    }
}

async fn get(addr: std::net::SocketAddr, path: &str) -> Result<serde_json::Value> {
    let response = timeout(
        Duration::from_secs(20),
        reqwest::Client::new()
            .get(format!("http://{addr}{path}"))
            .header("HOST", "race.test")
            .send(),
    )
    .await
    .context(format!("{path} timed out"))??;
    let status = response.status().as_u16();
    let body = response.text().await?;
    anyhow::ensure!(status == 200, "{path}: status {status}: {body}");
    eprintln!("{path} -> {body}");
    serde_json::from_str(&body).with_context(|| format!("{path}: bad json {body}"))
}

fn http_iface() -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "http".to_string(),
        interfaces: ["incoming-handler".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.2.2").unwrap()),
        config: HashMap::from([("host".to_string(), "race.test".to_string())]),
        name: None,
    }
}

fn req(
    components: Vec<Component>,
    volumes: Vec<wash_runtime::types::Volume>,
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "ctx-review".to_string(),
            annotations: HashMap::new(),
            service: None,
            components,
            host_interfaces: vec![http_iface()],
            volumes,
        },
    }
}

fn with_mount(mut c: Component, volume: &str, path: &str) -> Component {
    c.local_resources.volume_mounts = vec![wash_runtime::types::VolumeMount {
        name: volume.to_string(),
        mount_path: path.to_string(),
        read_only: true,
    }];
    c
}

/// A -> B -> C, all in A's store, B's call parked while A runs and C's call
/// parked while B runs. Each hop must observe its own WHO and its own
/// allowedHosts: A none (403), B example.com (502), C who=callee.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_three_hop_calls_keep_each_hops_context() -> Result<()> {
    let (addr, host) = start_host().await?;
    host.workload_start(req(
        vec![
            component("caller", CALLER_WASM, &[]),
            component("middle", MIDDLE_WASM, &["example.com"]),
            component("callee", CALLEE_WASM, &[]),
        ],
        vec![],
    ))
    .await?;
    let v = get(addr, "/nested").await?;
    let next = get(addr, "/whoami").await?;
    let mut problems = vec![];
    for (what, snap, who, egress) in [
        ("A during", &v["during"], "caller", 403),
        ("A after", &v["after"], "caller", 403),
        ("A next request", &next, "caller", 403),
        ("B before", &v["b"]["before"], "middle", 502),
        ("B during C", &v["b"]["during"], "middle", 502),
        ("B after C", &v["b"]["after"], "middle", 502),
    ] {
        if snap["who"] != who || snap["egress"] != egress {
            problems.push(format!("{what}: {snap} (want {who}/{egress})"));
        }
    }
    if v["b"]["c"] != "callee/callee" {
        problems.push(format!("C saw {}", v["b"]["c"]));
    }
    assert!(problems.is_empty(), "{}\nfull: {v}", problems.join("\n"));
    Ok(())
}

/// Volume isolation on both sides of a nested linked chain: the ROOT must not
/// see linked components' mounts either, and C two hops down sees only its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_preopens_are_per_component_on_both_sides() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mk = |n: &str| {
        let p = dir.path().join(n);
        std::fs::create_dir_all(&p).unwrap();
        wash_runtime::types::Volume {
            name: n.to_string(),
            volume_type: wash_runtime::types::VolumeType::HostPath(
                wash_runtime::types::HostPathVolume {
                    local_path: p.display().to_string(),
                },
            ),
        }
    };
    let vols = vec![mk("a"), mk("b"), mk("c")];
    let (addr, host) = start_host().await?;
    host.workload_start(req(
        vec![
            with_mount(component("caller", CALLER_WASM, &[]), "a", "/callerdir"),
            with_mount(
                component("middle", MIDDLE_WASM, &["example.com"]),
                "b",
                "/middir",
            ),
            with_mount(component("callee", VOL_CALLEE_WASM, &[]), "c", "/workspace"),
        ],
        vols,
    ))
    .await?;
    let p = get(addr, "/preopens").await?;
    let v = get(addr, "/nested").await?;
    assert_eq!(
        p["preopens"], "/callerdir",
        "root saw linked components' mounts: {p}"
    );
    assert_eq!(
        v["b"]["c"], "/workspace;write=refused",
        "C saw other mounts: {v}"
    );
    Ok(())
}

/// `n` copies of the callee. A shared digest lets the engine compile the
/// bytes once instead of once per copy.
fn many(n: usize) -> Vec<Component> {
    (0..n)
        .map(|i| Component {
            digest: Some("sha256:ctx-race-callee-fixture".to_string()),
            ..component(&format!("c{i}"), CALLEE_WASM, &[])
        })
        .collect()
}

/// The cap counts components that can share a store: 257 components that
/// never link to one another start (more than a workload could hold before),
/// and a linked group of 256 is refused with a clear error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn component_cap_boundary() -> Result<()> {
    let (_addr, host) = start_host().await?;
    let mut ok = req(many(257), vec![]);
    ok.workload.host_interfaces.clear();
    let t = std::time::Instant::now();
    let r = host.workload_start(ok).await.context("257 start call")?;
    eprintln!(
        "257 unlinked components -> {:?} {:?} in {:?}",
        r.workload_status.workload_state,
        r.workload_status.message,
        t.elapsed()
    );
    assert_eq!(
        r.workload_status.workload_state,
        wash_runtime::types::WorkloadState::Running
    );

    // One caller importing what 255 callees export: one linked group of 256.
    let mut linked = many(255);
    linked.push(component("caller", GATE_CALLER_WASM, &[]));
    let mut over = req(linked, vec![]);
    over.workload.host_interfaces.clear();
    let r = host
        .workload_start(over)
        .await
        .context("256 linked start call")?;
    eprintln!(
        "256 linked components -> {:?} {:?}",
        r.workload_status.workload_state, r.workload_status.message
    );
    assert_eq!(
        r.workload_status.workload_state,
        wash_runtime::types::WorkloadState::Error
    );
    assert!(
        r.workload_status
            .message
            .contains("at most 255 components that link to one another"),
        "{:?}",
        r.workload_status.message
    );
    Ok(())
}
