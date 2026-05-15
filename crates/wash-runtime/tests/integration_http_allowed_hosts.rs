//! Integration test for allowed_hosts policy on outgoing HTTP requests.
//!
//! Uses the http-allowed-hosts component which:
//! - `/example` makes an outgoing request to `example.com`
//! - `/org`     makes an outgoing request to `example.org` (unrelated domain)
//! - `/www`     makes an outgoing request to `www.example.com` (subdomain)
//!
//! All three targets are IANA-reserved (RFC 2606), so tests don't depend on
//! third-party bot-detection or rate limits. Having three lets us cover the
//! three distinct match outcomes:
//! - exact host match           (`/example` vs policy `example.com`)
//! - unrelated host             (`/org`     vs policy `example.com`)
//! - subdomain of policy host   (`/www`     vs policy `example.com` or `*.example.com`)
//!
//! The fixture reports the policy outcome via its own status, not the
//! upstream's:
//! - 200 OK          — upstream was reached (whatever upstream returned)
//! - 403 Forbidden   — denied by the host's allowed_hosts policy
//! - 502 Bad Gateway — DNS/network/TLS failure (treated as "egress unavailable")

#![allow(clippy::unwrap_used, clippy::expect_used)]

use anyhow::{Context, Result};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DynamicRouter, HttpServer},
    },
    plugin::{wasi_config::DynamicConfig, wasi_logging::TracingLogger},
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_ALLOWED_HOSTS_WASM: &[u8] = include_bytes!("wasm/http_allowed_hosts.wasm");

async fn start_host(addr: &str) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DynamicRouter::default(), addr.parse()?).await?;
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
                    allowed_hosts: allowed_hosts.into(),
                    ..Default::default()
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

    let status = example_response.status();
    // example.com is in the allowlist, so 200 (upstream reached) is the
    // success case. 502 covers CI runs where egress is unavailable — still
    // a pass since the host didn't reject the request on policy grounds.
    assert!(
        status.as_u16() == 200 || status.as_u16() == 502,
        "Request to example.com should be allowed (got {})",
        status
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

    let status = www_response.status();
    assert!(
        status.as_u16() == 200 || status.as_u16() == 502,
        "Request to www.example.com should be allowed by *.example.com wildcard (got {})",
        status
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

        let status = response.status();
        assert!(
            status.as_u16() == 200 || status.as_u16() == 502,
            "With allowed_hosts=['*'], {path} should not be blocked by policy (got {})",
            status
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
