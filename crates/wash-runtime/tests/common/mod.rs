//! Shared helpers for wash-runtime integration tests.

#![allow(dead_code)]
// Shared builder helpers unwrap/expect on constant fixtures in non-`#[test]`
// fns, which the clippy.toml in-tests allows don't cover. This module is
// included by many test crates, so keep the allow self-contained here rather
// than relying on every consumer to carry one.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[cfg(feature = "wasi-tls")]
pub mod tls;

#[cfg(feature = "wasmcloud-postgres")]
pub mod postgres;

use anyhow::{Context, Result};
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};
use tokio::time::timeout;

#[cfg(feature = "host-component-plugins")]
use wash_runtime::plugin::component_host::ComponentHostPlugin;
use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, DynamicRouter, HttpServer, TlsConfig},
    },
    plugin::{
        wasi_blobstore::InMemoryBlobstore, wasi_config::DynamicConfig,
        wasi_keyvalue::InMemoryKeyValue, wasi_logging::TracingLogger,
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

/// HTTP incoming-handler (0.2.2) with `host` config, plus optional
/// comma-separated `host-aliases`. (example :  "admin,user,customer")
pub fn http_incoming_handler_interface(host_header: &str, aliases: Option<&str>) -> WitInterface {
    let mut config = HashMap::new();
    config.insert("host".to_string(), host_header.to_string());
    if let Some(aliases) = aliases {
        config.insert("host-aliases".to_string(), aliases.to_string());
    }
    WitInterface {
        namespace: "wasi".to_string(),
        package: "http".to_string(),
        interfaces: ["incoming-handler".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.2.2").unwrap()),
        config,
        name: None,
    }
}

fn wasi_blobstore_interface() -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "blobstore".to_string(),
        interfaces: [
            "blobstore".to_string(),
            "container".to_string(),
            "types".to_string(),
        ]
        .into_iter()
        .collect(),
        version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

fn wasi_keyvalue_interface() -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "keyvalue".to_string(),
        interfaces: ["store".to_string(), "atomics".to_string()]
            .into_iter()
            .collect(),
        version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

fn wasi_logging_interface() -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "logging".to_string(),
        interfaces: ["logging".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0-draft").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

fn wasi_config_interface() -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "config".to_string(),
        interfaces: ["store".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.2.0-rc.1").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

/// Interfaces used by the `http-counter` component: HTTP, blobstore,
/// keyvalue, logging, config.
pub fn http_counter_host_interfaces(host_header: &str) -> Vec<WitInterface> {
    http_counter_host_interfaces_with_aliases(host_header, None)
}

/// Same as `http_counter_host_interfaces` but with optional host aliases are
/// passed through to the HTTP interface's `host-aliases` config entry.
pub fn http_counter_host_interfaces_with_aliases(
    host_header: &str,
    aliases: Option<&str>,
) -> Vec<WitInterface> {
    vec![
        http_incoming_handler_interface(host_header, aliases),
        wasi_blobstore_interface(),
        wasi_keyvalue_interface(),
        wasi_logging_interface(),
        wasi_config_interface(),
    ]
}

/// Interfaces for HTTP-only components (e.g. `http-handler-p2`,
/// `http-handler-p3`): just the HTTP incoming-handler interface.
pub fn http_only_host_interfaces(host_header: &str) -> Vec<WitInterface> {
    vec![http_incoming_handler_interface(host_header, None)]
}

/// The bespoke `acme:kv/store@0.1.0` capability, provided by the `kv-plugin`
/// host component plugin.
#[cfg(feature = "host-component-plugins")]
pub fn acme_kv_interface() -> WitInterface {
    WitInterface {
        namespace: "acme".to_string(),
        package: "kv".to_string(),
        interfaces: ["store".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

/// Interfaces for the `kv-plugin-caller` workload: HTTP ingress plus the imported
/// `acme:kv/store` capability the host component plugin satisfies.
#[cfg(feature = "host-component-plugins")]
pub fn kv_plugin_caller_host_interfaces(host_header: &str) -> Vec<WitInterface> {
    vec![
        http_incoming_handler_interface(host_header, None),
        acme_kv_interface(),
    ]
}

/// Interfaces for P3 HTTP + blobstore components.
pub fn http_blobstore_host_interfaces(host_header: &str) -> Vec<WitInterface> {
    vec![
        http_incoming_handler_interface(host_header, None),
        wasi_blobstore_interface(),
    ]
}

pub fn component_workload_request(
    component_name: &str,
    workload_name: &str,
    wasm: &'static [u8],
    local_resources: LocalResources,
    host_interfaces: Vec<WitInterface>,
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: workload_name.to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: component_name.to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(wasm),
                local_resources,
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces,
            volumes: vec![],
        },
    }
}

pub fn default_counter_resources() -> LocalResources {
    // http-counter calls example.com — encode that exact host in the
    // policy so the deny-all default doesn't block it AND the test
    // documents which upstream the fixture talks to.
    LocalResources {
        memory_limit_mb: 256,
        cpu_limit: 1,
        config: HashMap::new(),
        environment: HashMap::new(),
        volume_mounts: vec![],
        allowed_hosts: vec!["example.com".parse().unwrap()].into(),
    }
}

/// Attach the standard suite of plugins used by http-counter tests:
/// in-memory blobstore + keyvalue, tracing logger, dynamic config.
fn with_standard_plugins(
    builder: wash_runtime::host::HostBuilder,
) -> Result<wash_runtime::host::HostBuilder> {
    builder
        .with_plugin(Arc::new(InMemoryBlobstore::new(None)))?
        .with_plugin(Arc::new(InMemoryKeyValue::new()))?
        .with_plugin(Arc::new(TracingLogger::default()))?
        .with_plugin(Arc::new(DynamicConfig::default()))
}

/// Start a host with a "DevRouter" backed HTTP server and the standard plugin
/// set. Returns the bound address and a started `HostApi` ref.
pub async fn start_host_with_dev_router(
    addr: &str,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), addr.parse()?).await?;
    let bound_addr = http_server.addr();
    let host = with_standard_plugins(
        HostBuilder::new()
            .with_engine(engine)
            .with_http_handler(Arc::new(http_server)),
    )?
    .build()?;
    let host = host.start().await.context("Failed to start host")?;
    Ok((bound_addr, host))
}

/// Start a host with a "DynamicRouter" backed HTTP server and the standard
/// plugin set.
pub async fn start_host_with_dynamic_router(
    addr: &str,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DynamicRouter::default(), addr.parse()?).await?;
    let bound_addr = http_server.addr();
    let host = with_standard_plugins(
        HostBuilder::new()
            .with_engine(engine)
            .with_http_handler(Arc::new(http_server)),
    )?
    .build()?;
    let host = host.start().await.context("Failed to start host")?;
    Ok((bound_addr, host))
}

/// Start a host with a TLS-enabled `DevRouter`-backed HTTP server and the
/// standard plugin set. Certificate and key are read from disk at the given
/// paths.
pub async fn start_host_with_tls(
    cert_path: &Path,
    key_path: &Path,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new_with_tls(
        DevRouter::default(),
        "127.0.0.1:0".parse()?,
        TlsConfig::new(cert_path, key_path),
    )
    .await?;
    let bound_addr = http_server.addr();
    let host = with_standard_plugins(
        HostBuilder::new()
            .with_engine(engine)
            .with_http_handler(Arc::new(http_server)),
    )?
    .build()?;
    let host = host.start().await.context("Failed to start host")?;
    Ok((bound_addr, host))
}

/// Start a host with `wasip3` enabled on the engine, a `DevRouter` backed
/// HTTP server, and the standard plugin set.
pub async fn start_host_with_p3_http_handler(
    addr: &str,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), addr.parse()?).await?;
    let bound_addr = http_server.addr();
    let host = with_standard_plugins(
        HostBuilder::new()
            .with_engine(engine)
            .with_http_handler(Arc::new(http_server)),
    )?
    .build()?;
    let host = host.start().await.context("Failed to start host")?;
    Ok((bound_addr, host))
}

/// Start a p3 host with the standard plugin set plus a [`ComponentHostPlugin`]
/// built from `plugin_wasm`, routed by `router`, with `max_restarts` overriding
/// the plugin's supervision budget when given. The named wrappers below cover
/// the common shapes.
#[cfg(feature = "host-component-plugins")]
async fn start_host_with_component_plugin_router(
    addr: &str,
    router: impl wash_runtime::host::http::Router,
    plugin_id: &'static str,
    plugin_wasm: &'static [u8],
    max_restarts: Option<u32>,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(router, addr.parse()?).await?;
    let bound_addr = http_server.addr();
    let mut plugin = ComponentHostPlugin::new(plugin_id, plugin_wasm, engine.clone())
        .context("failed to build host component plugin")?;
    if let Some(max_restarts) = max_restarts {
        plugin = plugin.with_max_restarts(max_restarts);
    }
    let host = with_standard_plugins(
        HostBuilder::new()
            .with_engine(engine)
            .with_http_handler(Arc::new(http_server)),
    )?
    .with_plugin(Arc::new(plugin))?
    .build()?;
    let host = host.start().await.context("Failed to start host")?;
    Ok((bound_addr, host))
}

/// Start a p3 host with the standard plugin set plus a [`ComponentHostPlugin`]
/// built from `plugin_wasm` (a host component plugin exporting a capability).
/// Used to test workloads that import a component-provided host capability.
#[cfg(feature = "host-component-plugins")]
pub async fn start_host_with_component_plugin(
    addr: &str,
    plugin_id: &'static str,
    plugin_wasm: &'static [u8],
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    start_host_with_component_plugin_router(
        addr,
        DevRouter::default(),
        plugin_id,
        plugin_wasm,
        None,
    )
    .await
}

/// Like [`start_host_with_component_plugin`] but with a `DynamicRouter` that
/// routes by `Host` header — so distinct workloads are reachable individually
/// (the `DevRouter` sends every request to the last-resolved workload). Needed
/// to test per-caller behavior across genuinely separate workloads.
#[cfg(feature = "host-component-plugins")]
pub async fn start_host_with_component_plugin_by_host(
    addr: &str,
    plugin_id: &'static str,
    plugin_wasm: &'static [u8],
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    start_host_with_component_plugin_router(
        addr,
        DynamicRouter::default(),
        plugin_id,
        plugin_wasm,
        None,
    )
    .await
}

/// Like [`start_host_with_component_plugin`] but overriding the plugin's
/// supervision restart budget — for tests that exhaust it.
#[cfg(feature = "host-component-plugins")]
pub async fn start_host_with_component_plugin_max_restarts(
    addr: &str,
    plugin_id: &'static str,
    plugin_wasm: &'static [u8],
    max_restarts: u32,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    start_host_with_component_plugin_router(
        addr,
        DevRouter::default(),
        plugin_id,
        plugin_wasm,
        Some(max_restarts),
    )
    .await
}

/// Like [`start_host_with_p3_http_handler`] but also returns the [`HttpServer`], so a test can
/// drive host-side ingress hooks directly (e.g. deliver a message to a trigger service's
/// messaging handler via `deliver_trigger_service_message`).
pub async fn start_host_with_p3_handler(
    addr: &str,
) -> Result<(
    std::net::SocketAddr,
    impl HostApi,
    Arc<HttpServer<DevRouter>>,
)> {
    let engine = Engine::builder().build()?;
    let http_server = Arc::new(HttpServer::new(DevRouter::default(), addr.parse()?).await?);
    let bound_addr = http_server.addr();
    let host = with_standard_plugins(
        HostBuilder::new()
            .with_engine(engine)
            .with_http_handler(http_server.clone()),
    )?
    .build()?;
    let host = host.start().await.context("Failed to start host")?;
    Ok((bound_addr, host, http_server))
}

/// Extract the numeric value of `"name":N` from a flat JSON body without
/// pulling in a JSON dependency. Panics (failing the test) if the field is
/// missing or non-numeric.
pub fn json_u64_field(body: &str, name: &str) -> u64 {
    let key = format!("\"{name}\":");
    let start = body.find(&key).expect("field present in body") + key.len();
    let rest = &body[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().expect("numeric field")
}

/// GET `http://{addr}/` with the given `HOST` header and a 10s timeout.
pub async fn get_status(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    host_header: &str,
) -> Result<reqwest::StatusCode> {
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", host_header)
            .send(),
    )
    .await
    .with_context(|| format!("request to {host_header} timed out"))?
    .with_context(|| format!("request to {host_header} failed"))?;
    Ok(response.status())
}

/// Like `get_status` but also returns the response body text.
pub async fn get_status_and_body(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    host_header: &str,
) -> Result<(reqwest::StatusCode, String)> {
    let response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", host_header)
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}
