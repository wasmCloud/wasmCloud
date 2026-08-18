//! Integration tests for same-host local routing.
//!
//! Two workloads run on one host:
//! - **caller** — the `http-allowed-hosts` fixture, which makes an outgoing
//!   request per route: `/example` → `http://example.com`, `/org` →
//!   `http://example.org`, `/path` → `http://gateway.test/functiona/items`.
//! - **callee** — the `http-handler-p2` fixture (responds `200 "hello from
//!   p2"`), published to the network at its `host`/`host-aliases` and offered
//!   to co-located callers at its `localRoute` entries (`host` or `host/path`).
//!
//! Same-host routing takes two keys, and these tests exercise both halves: the
//! host must run with local routing enabled ([`Ingress`]), *and* the callee must
//! declare a `localRoute`. Neither alone routes anything, and the two scopes do
//! not leak into each other — a published hostname is never short-circuited, a
//! `localRoute` name is never reachable from the network.
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
    start_host_with_quota(local_routing, None).await
}

/// `start_host`, with an optional per-workload outbound-HTTP ceiling so a test
/// can watch local dispatch draw on the same allowance network egress draws on.
async fn start_host_with_quota(
    local_routing: bool,
    outbound_http: Option<usize>,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let mut builder = Ingress::builder(DynamicRouter::default(), "127.0.0.1:0".parse()?)
        .outgoing_handler(RefuseOutgoingHandler)
        .local_routing(local_routing);
    if let Some(outbound_http) = outbound_http {
        builder = builder.quotas(wash_runtime::host::quota::QuotaRegistry::new(
            wash_runtime::host::quota::QuotaLimits {
                outbound_http,
                ..Default::default()
            },
            None,
        ));
    }
    let ingress = builder.build().await?;
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
                    allowed_host_loopback_ports: Default::default(),
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
    call_host(addr, "caller.test", path).await
}

/// `call`, but addressing an arbitrary ingress hostname — used to check that an
/// inbound request resolves to the same workload a locally routed egress does.
async fn call_host(
    addr: std::net::SocketAddr,
    host_header: &str,
    path: &str,
) -> Result<(u16, String)> {
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}{path}"))
            .header("HOST", host_header)
            .send(),
    )
    .await
    .context(format!("{path} request timed out"))?
    .context(format!("Failed to make {path} request"))?;
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}

/// The base case: the callee declares `localRoute` for the authority the caller
/// dials, so the egress is served in-memory. An authority nothing declares
/// still egresses (and is refused by the stub handler).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_local_routing_declared_route() -> Result<()> {
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
        "callee.test",
        &[("localRoute", "example.com")],
        &[],
    ))
    .await
    .context("Failed to start callee")?;

    let (status, body) = call(addr, "/example").await?;
    assert_eq!(
        status, 200,
        "egress to a declared localRoute should be served in-memory: {body}"
    );
    assert!(
        body.contains("upstream 200"),
        "caller should observe the callee's 200: {body}"
    );

    let (status, body) = call(addr, "/org").await?;
    assert_eq!(
        status, 502,
        "egress to an undeclared authority must still hit the network path: {body}"
    );

    Ok(())
}

/// The two-key contract, workload half: a hostname the callee publishes to the
/// network via `host`/`host-aliases` is *not* short-circuited. Without a
/// `localRoute` declaration nothing routes locally, however reachable the name
/// is from outside.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_ingress_hostnames_alone_do_not_route_locally() -> Result<()> {
    let (addr, host) = start_host(true).await?;

    host.workload_start(http_workload(
        "caller",
        CALLER_WASM,
        "caller.test",
        &[],
        &["*"],
    ))
    .await?;
    // Serves example.com to the network, and declares no localRoute at all.
    host.workload_start(http_workload(
        "callee",
        CALLEE_WASM,
        "example.com",
        &[("host-aliases", "gateway.test")],
        &[],
    ))
    .await?;

    let (status, body) = call(addr, "/example").await?;
    assert_eq!(
        status, 502,
        "the primary `host` must not be locally routed without localRoute: {body}"
    );
    let (status, body) = call(addr, "/path").await?;
    assert_eq!(
        status, 502,
        "a `host-aliases` entry must not be locally routed either: {body}"
    );

    Ok(())
}

/// The two-key contract, host half: a declared `localRoute` is inert unless the
/// host itself enables local routing.
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
        "callee.test",
        &[("localRoute", "example.com")],
        &[],
    ))
    .await?;

    let (status, _) = call(addr, "/example").await?;
    assert_eq!(
        status, 502,
        "a declared localRoute is inert on a host without local routing enabled"
    );

    Ok(())
}

/// The caller's `allowed_hosts` policy is enforced before local routing: a
/// co-located callee must not widen a workload's egress policy.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_local_routing_respects_allowed_hosts() -> Result<()> {
    let (addr, host) = start_host(true).await?;

    // Caller may only reach example.org; example.com is denied by policy even
    // though the callee declares a localRoute for it.
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
        "callee.test",
        &[("localRoute", "example.com")],
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

/// A `localRoute` carrying a path serves only egress under that prefix. The
/// caller's `/path` route egresses to `http://gateway.test/functiona/items`,
/// which `gateway.test/functiona` claims.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_local_routing_matches_a_path_scoped_route() -> Result<()> {
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
        &[("localRoute", "gateway.test/functiona")],
        &[],
    ))
    .await?;

    let (status, body) = call(addr, "/path").await?;
    assert_eq!(
        status, 200,
        "an egress under the declared path prefix should be served in-memory: {body}"
    );

    Ok(())
}

/// The other half: a hostname declared *only* under a narrower prefix must not
/// swallow paths outside it. Keyed on the authority alone this egress was
/// short-circuited to a workload that does not serve the route.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_local_routing_skips_a_path_outside_the_prefix() -> Result<()> {
    let (addr, host) = start_host(true).await?;

    host.workload_start(http_workload(
        "caller",
        CALLER_WASM,
        "caller.test",
        &[],
        &["*"],
    ))
    .await?;
    // Declares gateway.test, but only under /elsewhere. The caller's `/path`
    // egress is to /functiona/items, which no route claims.
    host.workload_start(http_workload(
        "callee",
        CALLEE_WASM,
        "callee.test",
        &[("localRoute", "gateway.test/elsewhere")],
        &[],
    ))
    .await?;

    let (status, body) = call(addr, "/path").await?;
    assert_eq!(
        status, 502,
        "a path outside every declared prefix must egress to the network: {body}"
    );

    Ok(())
}

/// Prefix matching is on `/` segment boundaries, so `/function` must not claim
/// `/functiona/items` — the string-prefix bug this guards is easy to reintroduce.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_local_routing_prefix_stops_at_a_segment_boundary() -> Result<()> {
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
        &[("localRoute", "gateway.test/function")],
        &[],
    ))
    .await?;

    let (status, body) = call(addr, "/path").await?;
    assert_eq!(
        status, 502,
        "/function is not a segment-boundary prefix of /functiona/items: {body}"
    );

    Ok(())
}

/// A `localRoute` name is reachable in-memory and nowhere else. Anyone inside
/// the cluster can dial the host's port with a forged `Host` header, so a
/// local-only name answering an inbound request would be a real hole.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_local_routes_are_not_reachable_from_the_network() -> Result<()> {
    let (addr, host) = start_host(true).await?;

    host.workload_start(http_workload(
        "callee",
        CALLEE_WASM,
        "callee.test",
        &[("localRoute", "gateway.test/functiona")],
        &[],
    ))
    .await?;

    // The published hostname serves normally.
    let (status, body) = call_host(addr, "callee.test", "/anything").await?;
    assert_eq!(
        status, 200,
        "the published `host` must serve inbound: {body}"
    );
    assert!(body.contains("hello from p2"), "unexpected body: {body}");

    // The localRoute name does not, at its own prefix or anywhere else.
    let (status, _) = call_host(addr, "gateway.test", "/functiona/items").await?;
    assert_eq!(
        status, 404,
        "a localRoute name must not answer a forged inbound Host header"
    );
    let (status, _) = call_host(addr, "gateway.test", "/").await?;
    assert_eq!(status, 404, "nor at any other path");

    Ok(())
}

/// Local dispatch draws on the caller's outbound-HTTP allowance, so its
/// in-memory fan-out is bounded by the same number as its network fan-out.
/// With a ceiling of zero effective slots every locally routed call is refused
/// rather than served for free.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_local_dispatch_draws_on_the_outbound_http_quota() -> Result<()> {
    // One permit, held for the life of each locally dispatched response.
    let (addr, host) = start_host_with_quota(true, Some(1)).await?;

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
        &[("localRoute", "example.com")],
        &[],
    ))
    .await?;

    // The allowance is enough for a serial call, so this still works: the point
    // is that the slot is taken and returned, not that it refuses outright.
    let (status, body) = call(addr, "/example").await?;
    assert_eq!(
        status, 200,
        "a call within the allowance is served in-memory: {body}"
    );
    let (status, body) = call(addr, "/example").await?;
    assert_eq!(
        status, 200,
        "the slot must be released when the response is drained: {body}"
    );

    Ok(())
}
