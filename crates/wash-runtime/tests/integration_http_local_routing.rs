//! Integration tests for same-host local routing.
//!
//! Two workloads run on one host:
//! - **caller** — the `http-allowed-hosts` fixture, which makes an outgoing
//!   request per route: `/example` → `http://example.com`, `/org` →
//!   `http://example.org`, `/path` → `http://gateway.test/functiona/items`.
//! - **callee** — the `http-handler-p2` fixture (responds `200 "hello from
//!   p2"`), reachable at the hostname its `wasi:http/incoming-handler`
//!   interface config declares (`host`, plus any `host-aliases`).
//!
//! With local routing enabled on the [`Ingress`], an outgoing request whose
//! authority matches a hostname the ingress serves is dispatched to the
//! co-located workload in-memory — no extra per-workload declaration.
//!
//! Egress is stubbed with [`RefuseOutgoingHandler`], which fails every network
//! send with `ConnectionRefused`. The caller fixture maps that to 502 and a
//! policy denial to 403, so the caller's status is a three-way oracle:
//! - 200 — the outgoing request was served (only possible via the in-memory
//!   local route, since the network handler always refuses)
//! - 403 — denied by `allowed_hosts` policy
//! - 502 — egressed to the network (and was refused)
#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::timeout;

use wasmtime_wasi_http::p2::{
    HttpResult,
    body::HyperOutgoingBody,
    types::{HostFutureIncomingResponse, OutgoingRequestConfig},
};

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

const CALLER_WASM: &[u8] = include_bytes!("wasm/http_allowed_hosts.wasm");
const CALLEE_WASM: &[u8] = include_bytes!("wasm/http_handler_p2.wasm");

/// Test [`OutgoingHandler`] that refuses every network send. Any 200 the
/// caller reports therefore proves the request never reached the network.
struct RefuseOutgoingHandler;

impl OutgoingHandler for RefuseOutgoingHandler {
    fn send_request(
        &self,
        _workload_id: &str,
        _request: hyper::Request<HyperOutgoingBody>,
        _config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;
        let handle =
            wasmtime_wasi::runtime::spawn(async move { Ok(Err(ErrorCode::ConnectionRefused)) });
        Ok(HostFutureIncomingResponse::pending(handle))
    }

    fn send_request_p3(
        &self,
        _workload_id: &str,
        _request: hyper::Request<wash_runtime::host::http_p3::P3Body>,
        _options: Option<wasmtime_wasi_http::p3::RequestOptions>,
        _fut: wash_runtime::host::http_p3::P3RequestErrorFuture,
    ) -> wash_runtime::host::http_p3::P3SendFuture {
        use wasmtime_wasi_http::p3::bindings::http::types::ErrorCode;
        Box::new(async {
            Err(wasmtime_wasi::TrappableError::from(
                ErrorCode::ConnectionRefused,
            ))
        })
    }
}

async fn start_host(local_routing: bool) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::builder(DynamicRouter::default(), "127.0.0.1:0".parse()?)
        .outgoing_handler(RefuseOutgoingHandler)
        .local_routing(local_routing)
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

fn http_workload(
    name: &str,
    wasm: &'static [u8],
    host_header: &str,
    extra_http_config: &[(&str, &str)],
    allowed_hosts: &[&str],
) -> WorkloadStartRequest {
    let parsed: Vec<wash_runtime::host::allowed_hosts::AllowedHost> = allowed_hosts
        .iter()
        .map(|s| s.parse().expect("test gave invalid allowed_hosts entry"))
        .collect();
    let mut config = HashMap::new();
    config.insert("host".to_string(), host_header.to_string());
    for (k, v) in extra_http_config {
        config.insert((*k).to_string(), (*v).to_string());
    }
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: name.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: format!("{name}.wasm"),
                digest: None,
                bytes: bytes::Bytes::from_static(wasm),
                local_resources: LocalResources {
                    memory_limit_mb: 128,
                    cpu_limit: 1,
                    config: HashMap::new(),
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: parsed.into(),
                    allowed_ip_name_lookups: Default::default(),
                },
                pool_size: 1,
                max_invocations: 100,
                max_concurrency: 1,
            }],
            host_interfaces: vec![WitInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.2.2").unwrap()),
                config,
                name: None,
            }],
            volumes: vec![],
        },
    }
}

async fn call(addr: std::net::SocketAddr, path: &str) -> Result<(u16, String)> {
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}{path}"))
            .header("HOST", "caller.test")
            .send(),
    )
    .await
    .context(format!("{path} request timed out"))?
    .context(format!("Failed to make {path} request"))?;
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}

/// An egress whose authority is the callee's ingress `host` (`example.com`) is
/// short-circuited in-memory; a non-served authority still egresses (and is
/// refused by the stub handler).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_local_routing_served_host() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let (addr, host) = start_host(true).await?;

    host.workload_start(http_workload(
        "caller",
        CALLER_WASM,
        "caller.test",
        &[],
        &["*"],
    ))
    .await
    .context("Failed to start caller")?;
    host.workload_start(http_workload(
        "callee",
        CALLEE_WASM,
        "example.com",
        &[],
        &[],
    ))
    .await
    .context("Failed to start callee")?;

    let (status, body) = call(addr, "/example").await?;
    assert_eq!(
        status, 200,
        "egress to example.com should be served in-memory by the callee: {body}"
    );
    assert!(
        body.contains("upstream 200"),
        "caller should observe the callee's 200: {body}"
    );

    let (status, body) = call(addr, "/org").await?;
    assert_eq!(
        status, 502,
        "egress to a non-served authority must still hit the network path: {body}"
    );

    Ok(())
}

/// `host-aliases` route locally exactly like the primary `host`, and the
/// egress URL's path travels with the locally dispatched request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_local_routing_host_alias() -> Result<()> {
    let (addr, host) = start_host(true).await?;

    host.workload_start(http_workload(
        "caller",
        CALLER_WASM,
        "caller.test",
        &[],
        &["*"],
    ))
    .await?;
    host.workload_start(http_workload(
        "callee",
        CALLEE_WASM,
        "callee.test",
        &[("host-aliases", "gateway.test")],
        &[],
    ))
    .await?;

    // `/path` egresses to `http://gateway.test/functiona/items`: the alias
    // matches the authority; the non-root path rides along.
    let (status, body) = call(addr, "/path").await?;
    assert_eq!(
        status, 200,
        "an aliased hostname should serve the egress in-memory: {body}"
    );

    // Same callee, an authority it does not serve.
    let (status, _) = call(addr, "/example").await?;
    assert_eq!(
        status, 502,
        "a non-served authority must egress to the network"
    );

    Ok(())
}

/// Local routing is off by default: even when the callee serves the authority,
/// egress uses the network path unless the ingress opts in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_local_routing_disabled_by_default() -> Result<()> {
    let (addr, host) = start_host(false).await?;

    host.workload_start(http_workload(
        "caller",
        CALLER_WASM,
        "caller.test",
        &[],
        &["*"],
    ))
    .await?;
    host.workload_start(http_workload(
        "callee",
        CALLEE_WASM,
        "example.com",
        &[],
        &[],
    ))
    .await?;

    let (status, _) = call(addr, "/example").await?;
    assert_eq!(
        status, 502,
        "with local routing disabled the egress must use the (refusing) network path"
    );

    Ok(())
}

/// The caller's `allowed_hosts` policy is enforced before local routing: a
/// co-located callee must not widen a workload's egress policy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_local_routing_respects_allowed_hosts() -> Result<()> {
    let (addr, host) = start_host(true).await?;

    // Caller may only reach example.org; example.com is denied by policy even
    // though the callee serves it locally.
    host.workload_start(http_workload(
        "caller",
        CALLER_WASM,
        "caller.test",
        &[],
        &["example.org"],
    ))
    .await?;
    host.workload_start(http_workload(
        "callee",
        CALLEE_WASM,
        "example.com",
        &[],
        &[],
    ))
    .await?;

    let (status, _) = call(addr, "/example").await?;
    assert_eq!(
        status, 403,
        "allowed_hosts must deny the egress before local routing is consulted"
    );

    Ok(())
}
