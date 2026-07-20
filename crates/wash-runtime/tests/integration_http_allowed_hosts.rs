//! Integration test for allowed_hosts policy on outgoing HTTP requests.
//!
//! Uses the http-allowed-hosts component which:
//! - `/example` makes an outgoing request to `example.com`
//! - `/org`     makes an outgoing request to `example.org` (unrelated domain)
//! - `/www`     makes an outgoing request to `www.example.com` (subdomain)
//!
//! [`FakeOutgoingHandler`] intercepts the egress, so these three names are
//! never actually resolved or dialed — they exist purely as distinct authorities
//! that exercise the three match outcomes the policy matcher distinguishes:
//! - exact host match           (`/example` vs policy `example.com`)
//! - unrelated host             (`/org`     vs policy `example.com`)
//! - subdomain of policy host   (`/www`     vs policy `example.com` or `*.example.com`)
//!
//! The fixture reports the policy outcome via its own status, not the
//! upstream's:
//! - 200 OK — request was permitted by policy and reached the outgoing
//!   handler (which the tests stub to a synthetic 200 — see
//!   [`FakeOutgoingHandler`] — so the real network is never touched and runs
//!   are deterministic)
//! - 403 Forbidden — denied by the host's allowed_hosts policy
//! - 502 Bad Gateway — any other client error from the fixture's perspective

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::timeout;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use wasmtime_wasi_http::p2::{
    HttpResult,
    body::{HyperIncomingBody, HyperOutgoingBody},
    types::{HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig},
};

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DynamicRouter, HttpServer, OutgoingHandler},
    },
    plugin::{wasi_config::DynamicConfig, wasi_logging::TracingLogger},
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_ALLOWED_HOSTS_WASM: &[u8] = include_bytes!("wasm/http_allowed_hosts.wasm");

/// Test [`OutgoingHandler`] that synthesizes a 200 OK without dialing the
/// network. The runtime checks `allowed_hosts` *before* invoking the handler,
/// so denied requests never reach here — they short-circuit with
/// `HttpRequestDenied`, which the fixture maps to 403. That makes the
/// allowed-vs-denied distinction the only thing the upstream affects, and
/// removes external connectivity from the loop.
///
/// The request body is drained before responding: the wasi-side outgoing-body
/// writer is the sender for a channel whose receiver lives inside the request
/// we were handed, so dropping the request immediately would close the channel
/// and the guest's body-write would fail with `StreamError::Closed`, surfacing
/// as a non-policy error in the fixture.
struct FakeOutgoingHandler;

impl OutgoingHandler for FakeOutgoingHandler {
    fn send_request(
        &self,
        _workload_id: &str,
        request: hyper::Request<HyperOutgoingBody>,
        _config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        let handle = wasmtime_wasi::runtime::spawn(async move {
            let (_parts, body) = request.into_parts();
            // Surface drain errors so future tests sending non-empty bodies
            // don't silently mask a guest-side body-stream bug behind a 200.
            if let Err(e) = body.collect().await {
                tracing::warn!(error = ?e, "FakeOutgoingHandler: draining request body failed");
            }

            let body: HyperIncomingBody = Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed_unsync();
            let resp = hyper::Response::builder()
                .status(hyper::StatusCode::OK)
                .body(body)
                .expect("static response is well-formed");
            Ok(Ok(IncomingResponse {
                resp,
                worker: None,
                between_bytes_timeout: Duration::from_secs(1),
            }))
        });
        Ok(HostFutureIncomingResponse::pending(handle))
    }

    fn send_request_p3(
        &self,
        _workload_id: &str,
        _request: hyper::Request<wash_runtime::host::http_p3::P3Body>,
        _options: Option<wasmtime_wasi_http::p3::RequestOptions>,
        _fut: wash_runtime::host::http_p3::P3RequestErrorFuture,
    ) -> wash_runtime::host::http_p3::P3SendFuture {
        unimplemented!("fixture targets wasip2; P3 path is unused by these tests")
    }
}

async fn start_host(addr: &str) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::builder(DynamicRouter::default(), addr.parse()?)
        .outgoing_handler(FakeOutgoingHandler)
        .build()
        .await?;
    let bound_addr = http_server.addr();
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(TracingLogger::default()))?
        .with_plugin(Arc::new(DynamicConfig::default()))?
        .build()?;

    let host = host.start().await.context("Failed to start host")?;
    Ok((bound_addr, host))
}

fn allowed_hosts_workload(allowed_hosts: Vec<String>) -> WorkloadStartRequest {
    let parsed: Vec<wash_runtime::host::allowed_hosts::AllowedHost> = allowed_hosts
        .iter()
        .map(|s| s.parse().expect("test gave invalid allowed_hosts entry"))
        .collect();
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "http-allowed-hosts".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "http-allowed-hosts.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(HTTP_ALLOWED_HOSTS_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 128,
                    cpu_limit: 1,
                    config: HashMap::new(),
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: parsed.into(),
                    allow_ip_name_lookup: false,
                },
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: vec![WitInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.2.2").unwrap()),
                config: {
                    let mut config = HashMap::new();
                    config.insert("host".to_string(), "test".to_string());
                    config
                },
                name: None,
            }],
            volumes: vec![],
        },
    }
}

/// Exact `example.com` allows `/example` but blocks `/org` (unrelated domain)
/// and `/www` (subdomain). Locks two invariants of the bare-authority
/// variant: it doesn't permit unrelated hosts, and it doesn't implicitly
/// expand into a subdomain wildcard.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_allowed_hosts_blocks_denied_host() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let (addr, host) = start_host("127.0.0.1:0").await?;

    let req = allowed_hosts_workload(vec!["example.com".to_string()]);
    host.workload_start(req)
        .await
        .context("Failed to start workload")?;

    let client = reqwest::Client::new();

    for (path, why) in [
        ("/org", "unrelated host (example.org)"),
        (
            "/www",
            "subdomain (www.example.com) — bare authority is exact, not a wildcard",
        ),
    ] {
        let response = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}{path}"))
                .header("HOST", "test")
                .send(),
        )
        .await
        .context(format!("{path} request timed out"))?
        .context(format!("Failed to make {path} request"))?;

        assert_eq!(
            response.status().as_u16(),
            403,
            "{path} should be blocked by policy `[example.com]`: {why}"
        );
    }

    // /example should succeed (example.com is in allowed_hosts)
    let example_response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/example"))
            .header("HOST", "test")
            .send(),
    )
    .await
    .context("Example request timed out")?
    .context("Failed to make example request")?;

    assert_eq!(
        example_response.status().as_u16(),
        200,
        "example.com should be allowed by policy `[example.com]`"
    );

    Ok(())
}

/// Wildcard `*.example.com` allows the `www.example.com` subdomain but blocks
/// both the bare `example.com` (wildcard requires a non-empty prefix) and
/// `example.org` (unrelated domain).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_allowed_hosts_wildcard() -> Result<()> {
    let (addr, host) = start_host("127.0.0.1:0").await?;

    let req = allowed_hosts_workload(vec!["*.example.com".to_string()]);
    host.workload_start(req)
        .await
        .context("Failed to start workload")?;

    let client = reqwest::Client::new();

    // /www targets www.example.com — should be ALLOWED by wildcard
    let www_response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/www"))
            .header("HOST", "test")
            .send(),
    )
    .await
    .context("www request timed out")?
    .context("Failed to make www request")?;

    assert_eq!(
        www_response.status().as_u16(),
        200,
        "www.example.com should be allowed by wildcard `[*.example.com]`"
    );

    for (path, why) in [
        (
            "/example",
            "bare example.com (wildcard requires a non-empty prefix)",
        ),
        ("/org", "example.org (unrelated domain)"),
    ] {
        let response = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}{path}"))
                .header("HOST", "test")
                .send(),
        )
        .await
        .context(format!("{path} request timed out"))?
        .context(format!("Failed to make {path} request"))?;

        assert_eq!(
            response.status().as_u16(),
            403,
            "{path} should be blocked by policy `[*.example.com]`: {why}"
        );
    }

    Ok(())
}

/// Literal `*` (AllowedHost::Any) lets every host through, same as
/// an empty list but exercises the explicit `Any` variant rather than
/// the empty-list shortcut. Important because the wash config layer
/// resolves missing `allowed_hosts` to `[Any]` rather than an empty list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_star_any_permits_all() -> Result<()> {
    let (addr, host) = start_host("127.0.0.1:0").await?;

    let req = allowed_hosts_workload(vec!["*".to_string()]);
    host.workload_start(req)
        .await
        .context("Failed to start workload")?;

    let client = reqwest::Client::new();
    for path in ["/www", "/example", "/org"] {
        let response = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}{path}"))
                .header("HOST", "test")
                .send(),
        )
        .await
        .context(format!("{path} request timed out"))?
        .context(format!("Failed to make {path} request"))?;

        assert_eq!(
            response.status().as_u16(),
            200,
            "With allowed_hosts=['*'], {path} should be allowed"
        );
    }
    Ok(())
}

/// URL-form policy pins scheme. `/example` hits `http://example.com`; the
/// policy below allows `https://example.com` only. The request should be
/// blocked because the schemes differ — the host alone isn't enough.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_url_policy_pins_scheme() -> Result<()> {
    let (addr, host) = start_host("127.0.0.1:0").await?;

    let req = allowed_hosts_workload(vec!["https://example.com".to_string()]);
    host.workload_start(req)
        .await
        .context("Failed to start workload")?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/example"))
            .header("HOST", "test")
            .send(),
    )
    .await
    .context("Example request timed out")?
    .context("Failed to make example request")?;

    assert_eq!(
        response.status().as_u16(),
        403,
        "http://example.com should be blocked when policy is https://example.com"
    );
    Ok(())
}

/// An empty `allowed_hosts` list denies all outgoing requests.
/// Callers that want unrestricted egress must use the explicit `["*"]` form,
/// which the wash config layer applies automatically when `allowedHosts` is
/// omitted from YAML (see [`test_star_any_permits_all`]).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_empty_allowed_hosts_denies_all() -> Result<()> {
    let (addr, host) = start_host("127.0.0.1:0").await?;

    // Empty allowed_hosts = deny all egress.
    let req = allowed_hosts_workload(vec![]);
    host.workload_start(req)
        .await
        .context("Failed to start workload")?;

    let client = reqwest::Client::new();

    // Both routes should be BLOCKED by the empty-list deny-all policy.
    for path in ["/www", "/example", "/org"] {
        let response = timeout(
            Duration::from_secs(10),
            client
                .get(format!("http://{addr}{path}"))
                .header("HOST", "test")
                .send(),
        )
        .await
        .context(format!("{path} request timed out"))?
        .context(format!("Failed to make {path} request"))?;

        assert_eq!(
            response.status().as_u16(),
            403,
            "Empty allowed_hosts should deny all egress; {path} unexpectedly succeeded"
        );
    }

    Ok(())
}
