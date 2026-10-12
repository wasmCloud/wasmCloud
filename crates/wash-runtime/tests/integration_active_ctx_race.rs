//! Regression tests for the store-global `active_ctx` slot.
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
const CALLEE_WASM: &[u8] = include_bytes!("wasm/ctx_race_callee_p3.wasm");

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

fn workload() -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "ctx-race".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                component("caller", CALLER_WASM, &[]),
                component("callee", CALLEE_WASM, &["example.com"]),
            ],
            host_interfaces: vec![WitInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.2.2").unwrap()),
                config: HashMap::from([("host".to_string(), "race.test".to_string())]),
                name: None,
            }],
            volumes: vec![],
        },
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

fn assert_caller(snapshot: &serde_json::Value, what: &str) {
    assert_eq!(
        snapshot["who"], "caller",
        "{what}: the caller's wasi:cli/environment resolved to another component's context: {snapshot}"
    );
    assert_eq!(
        snapshot["egress"], 403,
        "{what}: the caller's outbound request was judged against another component's \
         allowedHosts (it is granted none): {snapshot}"
    );
}

async fn started() -> Result<(std::net::SocketAddr, impl HostApi)> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let (addr, host) = start_host().await?;
    host.workload_start(workload())
        .await
        .context("failed to start workload")?;
    Ok((addr, host))
}

/// Sanity: with no linked call in flight, the caller is the caller.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn baseline_caller_context() -> Result<()> {
    let (addr, _host) = started().await?;
    assert_caller(&get(addr, "/whoami").await?, "/whoami");
    Ok(())
}

/// One linked call parked in the callee while the caller keeps running: the
/// caller's own host calls must not resolve to the callee's context.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inflight_linked_call_does_not_lend_callee_context() -> Result<()> {
    let (addr, _host) = started().await?;
    let v = get(addr, "/inflight").await?;
    assert_eq!(
        v["callee"], "callee/callee",
        "callee saw the wrong context: {v}"
    );
    assert_caller(&v["during"], "while a linked call was in flight");
    assert_caller(&v["after"], "after the linked call returned");
    Ok(())
}

/// Two linked calls in flight at once, finished first-in first-out. The LIFO
/// save/restore leaves `active_ctx` on the callee after both return, and the
/// pooled store keeps it for the caller's next request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overlapping_linked_calls_restore_caller_context() -> Result<()> {
    let (addr, _host) = started().await?;
    let v = get(addr, "/overlap").await?;
    let next = get(addr, "/whoami").await?;
    let mut problems = Vec::new();
    for (k, want) in [("r1", "callee/callee"), ("r2", "callee/callee")] {
        if v[k] != want {
            problems.push(format!(
                "callee call {k} saw context {} (want {want})",
                v[k]
            ));
        }
    }
    for (what, snap) in [
        ("caller after both calls", &v["after"]),
        ("next request", &next),
    ] {
        if snap["who"] != "caller" || snap["egress"] != 403 {
            problems.push(format!("{what}: {snap} (want who=caller, egress=403)"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    Ok(())
}
