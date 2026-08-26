//! A host started with metering on must still serve HTTP.
//!
//! Metering wraps every guest call in `ExecutionTimeMeter::observe`, and every
//! host the wasmCloud chart deploys runs with `--enable-meters`, so this is the
//! configuration that matters. The meter samples the epoch callback and no
//! longer touches fuel, but `EngineBuilder::with_fuel_consumption` remains
//! public — and a store on a fuel-enabled engine starts at *zero* fuel while
//! instantiation runs guest code, so a path that builds a store without
//! priming it traps before the guest runs at all, surfacing as an empty 500
//! with no guest logs. The last test here holds that line.

use std::collections::HashMap;

use anyhow::{Context, Result};

use wash_runtime::{
    host::HostApi,
    types::{LocalResources, WorkloadStartRequest},
};

mod common;
use common::{
    component_workload_request, default_counter_resources, get_status_and_body,
    http_counter_host_interfaces, http_only_host_interfaces, start_host_with_fuel,
    start_host_with_meters,
};

const HTTP_HANDLER_P2_WASM: &[u8] = include_bytes!("wasm/http_handler_p2.wasm");
const HTTP_HANDLER_P3_WASM: &[u8] = include_bytes!("wasm/http_handler_p3.wasm");
const HTTP_COUNTER_WASM: &[u8] = include_bytes!("wasm/http_counter.wasm");

fn handler_request(
    name: &'static str,
    wasm: &'static [u8],
    host_header: &str,
) -> WorkloadStartRequest {
    component_workload_request(
        name,
        name,
        wasm,
        LocalResources {
            memory_limit_mb: 128,
            cpu_limit: 1,
            config: HashMap::new(),
            environment: HashMap::new(),
            volume_mounts: vec![],
            allowed_hosts: Default::default(),
            allowed_ip_name_lookups: Default::default(),
            allowed_host_loopback_ports: Default::default(),
        },
        http_only_host_interfaces(host_header),
    )
}

/// The regression: with metering on, a P2 component served nothing at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p2_http_is_served_with_metering_on() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let (addr, host) = start_host_with_meters("127.0.0.1:0").await?;
    host.workload_start(handler_request(
        "http-handler-p2.wasm",
        HTTP_HANDLER_P2_WASM,
        "metered",
    ))
    .await
    .context("failed to start the p2 handler")?;

    let client = reqwest::Client::new();
    let (status, body) = get_status_and_body(&client, addr, "metered").await?;
    assert!(
        status.is_success(),
        "a metered host must still serve p2 http, got {status} with body {body:?}"
    );
    Ok(())
}

/// P3 goes down a different dispatch path, and must survive the same host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn p3_http_is_served_with_metering_on() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let (addr, host) = start_host_with_meters("127.0.0.1:0").await?;
    host.workload_start(handler_request(
        "http-handler-p3.wasm",
        HTTP_HANDLER_P3_WASM,
        "metered-p3",
    ))
    .await
    .context("failed to start the p3 handler")?;

    let client = reqwest::Client::new();
    let (status, body) = get_status_and_body(&client, addr, "metered-p3").await?;
    assert!(
        status.is_success(),
        "a metered host must still serve p3 http, got {status} with body {body:?}"
    );
    Ok(())
}

/// A component that also drives host plugins (keyvalue, blobstore, logging,
/// config) exercises the linked-store path as well as the request store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_plugin_using_component_is_served_with_metering_on() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let (addr, host) = start_host_with_meters("127.0.0.1:0").await?;
    host.workload_start(component_workload_request(
        "http-counter.wasm",
        "http-counter-workload",
        HTTP_COUNTER_WASM,
        default_counter_resources(),
        http_counter_host_interfaces("metered-counter"),
    ))
    .await
    .context("failed to start the counter")?;

    let client = reqwest::Client::new();
    let (status, body) = get_status_and_body(&client, addr, "metered-counter").await?;
    assert!(
        status.is_success(),
        "a metered host must still serve a plugin-using component, got {status} with body {body:?}"
    );
    Ok(())
}

/// Fuel is nobody's default any more, but a host built on a fuel-enabled engine
/// must still serve: the store-priming this needs is easy to delete once no
/// test covers it, and its absence is invisible until a guest fails to start.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_is_served_on_a_fuel_enabled_engine() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let (addr, host) = start_host_with_fuel("127.0.0.1:0").await?;
    host.workload_start(handler_request(
        "http-handler-p2.wasm",
        HTTP_HANDLER_P2_WASM,
        "fuelled",
    ))
    .await
    .context("failed to start the p2 handler")?;

    let client = reqwest::Client::new();
    let (status, body) = get_status_and_body(&client, addr, "fuelled").await?;
    assert!(
        status.is_success(),
        "a fuel-enabled host must still serve http, got {status} with body {body:?}"
    );
    Ok(())
}
