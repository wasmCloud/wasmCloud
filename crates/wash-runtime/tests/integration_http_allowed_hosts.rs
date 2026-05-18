//! Integration test for allowed_hosts policy on outgoing HTTP requests.
//!
//! Uses the http-allowed-hosts component which:
//! - `/example` makes an outgoing request to `example.com`
//! - `/wiki` makes an outgoing request to `en.wikipedia.org`

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
                    config: HashMap::new(),
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: parsed.into(),
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

/// Only example.com is allowed. `/wiki` (→ en.wikipedia.org) should be blocked,
/// `/example` (→ example.com) should succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_allowed_hosts_blocks_denied_host() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let (addr, host) = start_host("127.0.0.1:0").await?;

    // Only allow example.com — en.wikipedia.org should be blocked
    let req = allowed_hosts_workload(vec!["example.com".to_string()]);
    host.workload_start(req)
        .await
        .context("Failed to start workload")?;

    let client = reqwest::Client::new();

    // /wiki should be blocked by policy
    let wiki_response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/wiki"))
            .header("HOST", "test")
            .send(),
    )
    .await
    .context("Wiki request timed out")?
    .context("Failed to make wiki request")?;

    assert_eq!(
        wiki_response.status().as_u16(),
        500,
        "Request to en.wikipedia.org should be blocked"
    );

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
    // example.com should be reachable; 502 is acceptable if network is unavailable in CI
    assert!(
        status.is_success() || status.as_u16() == 502,
        "Request to example.com should be allowed (got {})",
        status
    );

    Ok(())
}

/// Wildcard *.wikipedia.org allows en.wikipedia.org but blocks example.com.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_allowed_hosts_wildcard() -> Result<()> {
    let (addr, host) = start_host("127.0.0.1:0").await?;

    // Allow *.wikipedia.org — en.wikipedia.org should pass,
    // but example.com should be blocked
    let req = allowed_hosts_workload(vec!["*.wikipedia.org".to_string()]);
    host.workload_start(req)
        .await
        .context("Failed to start workload")?;

    let client = reqwest::Client::new();

    // /wiki targets en.wikipedia.org — should be ALLOWED by wildcard
    let wiki_response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/wiki"))
            .header("HOST", "test")
            .send(),
    )
    .await
    .context("Wiki request timed out")?
    .context("Failed to make wiki request")?;

    let status = wiki_response.status();
    assert_ne!(
        status.as_u16(),
        500,
        "Request to en.wikipedia.org should be allowed by *.wikipedia.org wildcard"
    );

    // /example targets example.com — should be BLOCKED (not in *.wikipedia.org)
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
        500,
        "Request to example.com should be blocked when only *.wikipedia.org is allowed"
    );

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
    for path in ["/wiki", "/example"] {
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

        assert_ne!(
            response.status().as_u16(),
            500,
            "With allowed_hosts=['*'], {path} should not be blocked by policy"
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
        500,
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
    for path in ["/wiki", "/example"] {
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
            500,
            "Empty allowed_hosts should deny all egress; {path} unexpectedly succeeded"
        );
    }

    Ok(())
}
