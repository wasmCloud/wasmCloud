//! Volume-mount isolation for linked components in one store (handle-carrying
//! path). Shares its host setup with `integration_active_ctx_race`.
//!
//! One workload, two components linked in ONE store: `caller` (an HTTP
//! handler, `WHO=caller`, granted no outbound hosts) imports
//! `wasmcloud:ctx-race/gate` from `callee` (`WHO=callee`, granted
//! `example.com`). `gate.hold` carries a `future<u32>`, so in a plain
//! (non-service) workload it runs on the shared-store path
//! (`LinkedTarget::SharedStore`), the one that swaps `SharedCtx::active_ctx`.
//!
//! Every route reports what the caller's OWN host calls resolved to: its `WHO`
//! environment variable (`wasi:cli/environment`) and the policy decision on an
//! outgoing request to `example.com` (403 = denied by the caller's grants,
//! 502 = permitted and refused by the stub upstream). Both must always be the
//! caller's: `caller` / 403.
//!
//! The interleavings are driven by the guest, not by timing: the callee parks
//! on a future the caller resolves, so each test is deterministic.
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

const CALLER_WASM: &[u8] = include_bytes!("wasm/ctx_race_caller_p3.wasm");
const CALLEE_WASM: &[u8] = include_bytes!("wasm/vol_callee_p3.wasm");

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

fn with_mount(mut c: Component, volume: &str, path: &str) -> Component {
    c.local_resources.volume_mounts = vec![wash_runtime::types::VolumeMount {
        name: volume.to_string(),
        mount_path: path.to_string(),
        read_only: true,
    }];
    c
}

/// `caller` mounts `caller-dir` at /callerdir, `callee` mounts `tool-dir` at
/// /workspace, both read only. Each must see only its own mount: the root in
/// its own store, and the callee while it serves a `future`-carrying call on
/// the shared-store path, where a write under its read-only mount must fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn linked_callee_sees_only_its_own_volume_mounts() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let dir = tempfile::tempdir()?;
    let (caller_dir, tool_dir) = (dir.path().join("caller"), dir.path().join("tool"));
    std::fs::create_dir_all(&caller_dir)?;
    std::fs::create_dir_all(&tool_dir)?;
    let vol = |name: &str, p: &std::path::Path| wash_runtime::types::Volume {
        name: name.to_string(),
        volume_type: wash_runtime::types::VolumeType::HostPath(
            wash_runtime::types::HostPathVolume {
                local_path: p.display().to_string(),
            },
        ),
    };
    let (addr, host) = start_host().await?;
    host.workload_start(WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "vol-leak".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                with_mount(
                    component("caller", CALLER_WASM, &[]),
                    "caller-dir",
                    "/callerdir",
                ),
                with_mount(
                    component("callee", CALLEE_WASM, &[]),
                    "tool-dir",
                    "/workspace",
                ),
            ],
            host_interfaces: vec![WitInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.2.2").unwrap()),
                config: HashMap::from([("host".to_string(), "race.test".to_string())]),
                name: None,
            }],
            volumes: vec![vol("caller-dir", &caller_dir), vol("tool-dir", &tool_dir)],
        },
    })
    .await
    .context("failed to start workload")?;
    let root = get(addr, "/preopens").await?;
    assert_eq!(
        root["preopens"], "/callerdir",
        "the store's root saw a linked component's volume mounts: {root}"
    );
    let v = get(addr, "/inflight").await?;
    assert_eq!(
        v["callee"], "/workspace;write=refused",
        "the linked callee saw another component's volume mounts, or wrote to its \
         read-only one: {v}"
    );
    Ok(())
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
