//! Integration test: a host component plugin's own outgoing `wasi:http`
//! egress, secured by its own `allowedHosts` policy — independent of any
//! workload's.
//!
//! `wasi:http/outgoing-handler` links into a plugin's store unconditionally
//! once the plugin imports it; without an `allowed_hosts` policy and a real
//! HTTP handler on the plugin's own store, every outgoing call would simply
//! trap ("http client not available"). `http-egress-plugin` exports a bespoke
//! `acme:httpegress/fetch` capability that makes one outgoing GET and reports
//! the *policy* outcome (not the upstream's own status) as a string, driven
//! end to end over HTTP through the `http-egress-plugin-caller` workload.

#![cfg(feature = "host-component-plugins")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use tokio::time::timeout;
use wasmtime_wasi_http::p2::{
    HttpResult,
    body::{HyperIncomingBody, HyperOutgoingBody},
    types::{HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig},
};

use wash_runtime::engine::Engine;
use wash_runtime::host::http::{DevRouter, Ingress, OutgoingHandler};
use wash_runtime::host::{HostApi, HostBuilder};
use wash_runtime::plugin::component_host::ComponentHostPlugin;
use wash_runtime::types::LocalResources;
use wash_runtime::wit::WitInterface;

mod common;
use common::{component_workload_request, http_incoming_handler_interface};

const EGRESS_PLUGIN_WASM: &[u8] = include_bytes!("wasm/http_egress_plugin.wasm");
const CALLER_WASM: &[u8] = include_bytes!("wasm/http_egress_plugin_caller.wasm");
const PLUGIN_ID: &str = "http-egress-plugin";

/// Synthesizes a 200 OK without dialing the network — the runtime checks
/// `allowed_hosts` *before* invoking this handler, so a denied request never
/// reaches here (it short-circuits with `HttpRequestDenied`, which the
/// fixture maps to "403"). Mirrors `integration_http_allowed_hosts.rs`'s own
/// fake handler.
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
            let _ = body.collect().await;
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
        unimplemented!("this test only drives the plugin's p2 outgoing-handler import")
    }
}

fn fetch_interface() -> WitInterface {
    WitInterface {
        namespace: "acme".to_string(),
        package: "httpegress".to_string(),
        interfaces: ["fetch".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse("0.1.0").unwrap()),
        config: HashMap::new(),
        name: None,
    }
}

async fn req(client: &reqwest::Client, addr: &std::net::SocketAddr, host: &str) -> Result<String> {
    let resp = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/fetch?host={host}"))
            .header("HOST", "caller")
            .send(),
    )
    .await
    .context("request timed out")??;
    Ok(resp.text().await?)
}

/// Starts a host with a fake (network-free) outgoing handler and a single
/// `http-egress-plugin` registered with `allowed_hosts`, plus the caller
/// workload wired up to drive it.
async fn start_host_with_egress_plugin(
    allowed_hosts: Vec<wash_runtime::host::allowed_hosts::AllowedHost>,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let ingress = Ingress::builder(DevRouter::default(), "127.0.0.1:0".parse()?)
        .outgoing_handler(FakeOutgoingHandler)
        .build()
        .await?;
    let addr = ingress.addr();

    let builder = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(ingress));
    let native_plugins = builder.native_plugins();
    let http_handler = builder.http_handler();

    let plugin = ComponentHostPlugin::builder()
        .id(PLUGIN_ID)
        .wasm(EGRESS_PLUGIN_WASM)
        .engine(engine)
        .native_plugins(native_plugins)
        .allowed_hosts(allowed_hosts.into())
        .maybe_http_handler(http_handler)
        .build()
        .await
        .context("http-egress-plugin should link cleanly")?;

    let host = builder.with_plugin(Arc::new(plugin))?.build()?;
    let host = host.start().await.context("failed to start host")?;

    host.workload_start(component_workload_request(
        "http-egress-plugin-caller",
        "caller",
        CALLER_WASM,
        LocalResources::default(),
        vec![
            http_incoming_handler_interface("caller", None),
            fetch_interface(),
        ],
    ))
    .await?;

    Ok((addr, host))
}

/// A plugin's own `allowed_hosts` policy permits its own outgoing call to a
/// listed host.
#[tokio::test]
async fn test_plugin_egress_allowed_host_succeeds() -> Result<()> {
    let (addr, _host) = start_host_with_egress_plugin(vec!["example.com".parse()?]).await?;
    let client = reqwest::Client::new();

    let status = req(&client, &addr, "example.com").await?;
    assert_eq!(
        status, "200",
        "an allowed host must reach the (fake) upstream"
    );

    Ok(())
}

/// A plugin's own `allowed_hosts` policy denies its own outgoing call to a
/// host not on the list — independent of any workload's own policy, since
/// this call never touches a workload's store at all.
#[tokio::test]
async fn test_plugin_egress_denied_host_blocked() -> Result<()> {
    let (addr, _host) = start_host_with_egress_plugin(vec!["example.com".parse()?]).await?;
    let client = reqwest::Client::new();

    let status = req(&client, &addr, "example.org").await?;
    assert_eq!(
        status, "403",
        "a host outside the plugin's own allowed_hosts must be denied"
    );

    Ok(())
}

/// An empty `allowed_hosts` (the default when a plugin's spec doesn't set it)
/// denies every outgoing host — deny-by-default, matching a workload's
/// `LocalResources` default.
#[tokio::test]
async fn test_plugin_egress_empty_allowed_hosts_denies_all() -> Result<()> {
    let (addr, _host) = start_host_with_egress_plugin(vec![]).await?;
    let client = reqwest::Client::new();

    let status = req(&client, &addr, "example.com").await?;
    assert_eq!(
        status, "403",
        "an empty allowed_hosts policy must deny-by-default"
    );

    Ok(())
}
