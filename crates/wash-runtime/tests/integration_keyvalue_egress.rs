#![cfg(all(feature = "wasm_component_model_implements", feature = "wasi-keyvalue"))]
//! A `host.plugins` egress ceiling on the multiplexed keyvalue plugin.
//!
//! The `keyvalue-implements` fixture imports `wasi:keyvalue/store` as `team-a`
//! and `team-b`; each label's backend `url` comes from the workload. With
//! `allowedHosts` declared for `wasi-keyvalue-multiplexed`, a binding whose url
//! is outside the ceiling is refused at bind, before anything connects, and
//! one inside it connects and serves.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use testcontainers::{
    GenericImage,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::time::timeout;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        allowed_loopback::AllowedLoopbackPort,
        http::{DevRouter, Ingress},
    },
    plugin::{
        PluginBindingSet, PluginBindings, WorkloadConfigPolicy,
        wasi_keyvalue::{MultiplexedKeyValue, RedisProvider},
    },
    sockets::policy::SocketPolicy,
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

mod common;
use common::http_incoming_handler_interface;

const KEYVALUE_IMPLEMENTS_WASM: &[u8] = include_bytes!("wasm/keyvalue_implements.wasm");
const MULTIPLEXED_KEYVALUE_ID: &str = "wasi-keyvalue-multiplexed";

/// A labeled store on redis at `url`. Each label gets its own `prefix`, so the
/// fixture's isolation check holds on one server.
fn redis_store(name: &str, url: &str) -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
        config: HashMap::from([
            ("backend".to_string(), "redis".to_string()),
            ("url".to_string(), url.to_string()),
            ("prefix".to_string(), name.to_string()),
        ]),
        name: Some(name.to_string()),
    }
}

/// A host whose multiplexed keyvalue plugin may reach `cache.internal:6379`
/// and, on host loopback, only `loopback_port`.
async fn start_host(loopback_port: u16) -> Result<(impl HostApi, std::net::SocketAddr)> {
    let engine = Engine::builder()
        .with_socket_policy(Arc::new(SocketPolicy {
            host_loopback_enabled: true,
            ..Default::default()
        }))
        .build()?;
    let ingress = Ingress::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = ingress.addr();
    // `allow`, so each label's url is the workload's own: the ceiling is what
    // bounds it.
    let declared = PluginBindingSet::new(MULTIPLEXED_KEYVALUE_ID)
        .with_workload_config(WorkloadConfigPolicy::Allow)
        .with_egress_policy(
            Arc::from(["redis://cache.internal:6379".parse()?]),
            Arc::from([]),
            Arc::from([AllowedLoopbackPort::tcp(loopback_port)]),
        );
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(ingress))
        .with_plugin(Arc::new(
            MultiplexedKeyValue::new().with_provider(Arc::new(RedisProvider)),
        ))?
        .with_plugin_bindings(PluginBindings::new().with_plugin(declared))
        .build()
        .context("an entry declaring allowedHosts must start the host")?;
    let host = host.start().await.context("failed to start host")?;
    Ok((host, addr))
}

fn start_request(url: &str) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "keyvalue-egress".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "keyvalue-implements.wasm".to_string(),
                bytes: bytes::Bytes::from_static(KEYVALUE_IMPLEMENTS_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
                max_concurrency: 1,
                ..Default::default()
            }],
            host_interfaces: vec![
                http_incoming_handler_interface("kv-egress", None),
                redis_store("team-a", url),
                redis_store("team-b", url),
            ],
            volumes: vec![],
        },
    }
}

/// Refused before any connect, so no server is needed: neither name below
/// resolves, and a refusal that came from a failed connect would say so.
#[tokio::test]
async fn a_binding_outside_allowed_hosts_is_refused_at_bind() -> Result<()> {
    let (host, _) = start_host(6379).await?;
    for (url, refused) in [
        ("redis://evil.example:6379", "evil.example"),
        ("redis://:S3cret@evil.example:6379", "evil.example"),
        ("redis://cache.internal:6380", "cache.internal:6380"),
        ("redis://127.0.0.1:6380", "127.0.0.1:6380"),
    ] {
        // Bounded, so a check that let the bind reach a connect fails here
        // rather than hanging on DNS.
        let resp = timeout(
            Duration::from_secs(5),
            host.workload_start(start_request(url)),
        )
        .await
        .with_context(|| format!("{url}: refused before any connect"))??;
        let status = resp.workload_status;
        assert_ne!(status.workload_state, WorkloadState::Running, "{url}");
        assert!(
            status.message.contains("egress policy") && status.message.contains(refused),
            "{url}: the refusal names the policy and the endpoint: {}",
            status.message
        );
        assert!(
            !status.message.contains("S3cret"),
            "{url}: {}",
            status.message
        );
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker (redis); run with `cargo test --include-ignored`"]
async fn a_binding_inside_the_ceiling_connects() -> Result<()> {
    let redis = GenericImage::new("redis", "7-alpine")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start redis: {e}"))?;
    let port = redis.get_host_port_ipv4(6379).await?;
    let (host, addr) = start_host(port).await?;

    let resp = host
        .workload_start(start_request(&format!("redis://127.0.0.1:{port}")))
        .await?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Running,
        "{}",
        resp.workload_status.message
    );

    let response = timeout(
        Duration::from_secs(10),
        reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .header("HOST", "kv-egress")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    let status = response.status();
    let body = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {body}");
    assert_eq!(body, "isolated");
    Ok(())
}
