#![cfg(feature = "providers")]

use core::str;
use core::time::Duration;

use std::net::Ipv4Addr;

use anyhow::Context as _;
use tokio::task::JoinSet;
use tokio::time::sleep;
use tokio::try_join;
use tracing::info;
use tracing_subscriber::prelude::*;
use wasmcloud_core::tls::NativeRootsExt as _;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::provider::{assert_start_provider, StartProviderArgs};
use wasmcloud_test_util::{
    component::assert_scale_component, host::WasmCloudTestHost,
    lattice::link::assert_advertise_link,
};

use test_components::RUST_HTTP_EXPORT_TIMEOUT;

pub mod common;
use common::free_port;
use common::nats::start_nats;
use common::providers;

const LATTICE: &str = "default";
const COMPONENT_ID: &str = "http_component_export_timeout";

#[tokio::test]
async fn component_export_timeout() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    let (nats_server, nats_url, nats_client) =
        start_nats().await.context("failed to start NATS")?;

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client)
        .lattice(LATTICE.to_string())
        .build();
    // Build the host
    let host = WasmCloudTestHost::start_custom(
        &nats_url,
        LATTICE,
        None,
        None,
        None,
        None,
        Some(Duration::from_secs(1)),
    )
    .await
    .context("failed to start test host")?;

    let http_port = free_port().await?;

    let http_server_config_name = "http-server".to_string();

    let rust_http_server = providers::rust_http_server().await;
    let rust_http_server_id = rust_http_server.subject.public_key();
    let host_key = host.host_key();
    let host_id = host_key.public_key();
    try_join!(
        async {
            assert_config_put(
                &ctl_client,
                &http_server_config_name,
                [(
                    "ADDRESS".to_string(),
                    format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
                )],
            )
            .await
            .context("failed to put configuration")
        },
        async {
            let rust_http_server_url = rust_http_server.url();
            assert_start_provider(StartProviderArgs {
                client: &ctl_client,
                host_id: &host_id,
                provider_id: &rust_http_server_id,
                provider_ref: rust_http_server_url.as_str(),
                config: vec![],
            })
            .await
            .context("failed to start providers")
        },
        async {
            assert_scale_component(
                &ctl_client,
                &host_id,
                format!("file://{RUST_HTTP_EXPORT_TIMEOUT}"),
                COMPONENT_ID,
                None,
                5,
                Vec::new(),
                Duration::from_secs(10),
            )
            .await
            .context("failed to scale `rust-http-export-timeout` component")
        }
    )?;

    assert_advertise_link(
        &ctl_client,
        &rust_http_server_id,
        COMPONENT_ID,
        "default",
        "wasi",
        "http",
        vec!["incoming-handler".to_string()],
        vec![http_server_config_name],
        vec![],
    )
    .await
    .context("failed to advertise link")?;

    let http_client = reqwest::Client::builder()
        .with_native_certificates()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .context("failed to build HTTP client")?;

    // Wait for data to be propagated across lattice
    sleep(Duration::from_secs(1)).await;

    let url = format!("http://localhost:{http_port}/");
    let mut requests = JoinSet::new();

    for i in 0..10 {
        let req = http_client.get(&url).send();
        requests.spawn(async move {
            info!(i, "sending HTTP request");
            req.await
                .context("failed to connect to server")?
                .error_for_status()
                .context("failed to get response")?
                .text()
                .await
                .context("failed to get response text")
        });
    }

    // We expect these requests to fail due to the timeout
    let mut success_count = 0;
    let mut timeout_count = 0;

    for i in 0..10 {
        info!(i, "awaiting HTTP request");
        match tokio::time::timeout(Duration::from_secs(10), requests.join_next()).await {
            Ok(Some(Ok(Ok(res)))) => {
                info!(i, "received HTTP response");
                success_count += 1;
                assert_eq!(res, "Hello from Rust!\n");
            }
            Ok(Some(Err(e))) => {
                info!(i, error = %e, "request failed as expected due to timeout");
                timeout_count += 1;
            }
            Err(_) => {
                info!(i, "request timed out as expected");
                timeout_count += 1;
            }
            _ => panic!("Unexpected response state"),
        }
    }

    // Since we set a 1 second timeout in the host config and a 3 second sleep in the component,
    // we expect all requests to timeout
    assert!(
        timeout_count > 0 && success_count == 0,
        "Expected at least some requests to timeout"
    );

    nats_server.stop().await.context("failed to stop NATS")?;
    Ok(())
}
