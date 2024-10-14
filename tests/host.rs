#![cfg(feature = "providers")]

use core::str;
use core::time::Duration;

use std::net::Ipv4Addr;

use anyhow::{anyhow, Context};
use tokio::time::sleep;
use tokio::try_join;
use tracing_subscriber::prelude::*;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::provider::{assert_start_provider, StartProviderArgs};
use wasmcloud_test_util::{
    component::assert_scale_component, host::WasmCloudTestHost,
    lattice::link::assert_advertise_link,
};

use test_components::RUST_HTTP_HELLO_WORLD;

pub mod common;
use common::free_port;
use common::nats::start_nats;
use common::providers;

const LATTICES: &[&str] = &["default", "test-lattice"];
const COMPONENT_ID: &str = "http_hello_world";

#[tokio::test]
async fn host_multiple_lattices() -> anyhow::Result<()> {
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
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICES[0].to_string())
        .build();
    // Build the host
    let host =
        WasmCloudTestHost::start(&nats_url, LATTICES.iter().map(|v| v.to_string()).collect())
            .await
            .context("failed to start test host")?;

    let http_port = free_port().await?;

    let http_server_config_name = "http-server".to_string();

    let rust_http_server = providers::rust_http_server().await;
    let rust_http_server_id = rust_http_server.subject.public_key();

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
            let host_key = host.host_key();
            let rust_http_server_url = rust_http_server.url();
            assert_start_provider(StartProviderArgs {
                client: &ctl_client,
                lattice: LATTICES[0],
                host_key: &host_key,
                provider_key: &rust_http_server.subject,
                provider_id: &rust_http_server_id,
                url: &rust_http_server_url,
                config: vec![],
            })
            .await
            .context("failed to start providers")
        },
        async {
            assert_scale_component(
                &ctl_client,
                &host.host_key(),
                format!("file://{RUST_HTTP_HELLO_WORLD}"),
                COMPONENT_ID,
                None,
                5,
                Vec::new(),
            )
            .await
            .context("failed to scale `rust-http-hello-world` component")
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

    // Wait for data to be propagated across lattice
    sleep(Duration::from_secs(1)).await;

    let test_lattice_client = host.get_ctl_client(Some(nats_client), LATTICES[1]).await?;
    let inventory = test_lattice_client
        .get_host_inventory(&host.host_key().public_key())
        .await
        .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;
    let d = inventory.data().unwrap();
    assert_eq!(d.components().len(), 0);

    assert_scale_component(
        &test_lattice_client,
        &host.host_key(),
        format!("file://{RUST_HTTP_HELLO_WORLD}"),
        COMPONENT_ID,
        None,
        5,
        Vec::new(),
    )
    .await
    .context("failed to scale `rust-http-hello-world` component on test-lattice")?;
    let inventory_resp = test_lattice_client
        .get_host_inventory(&host.host_key().public_key())
        .await
        .map_err(|e| anyhow!(e).context("failed to get host inventory"))?;
    let inventory = inventory_resp.data().unwrap();
    // Check that the component started and there isn't a provider
    assert_eq!(inventory.components().len(), 1);
    assert_eq!(inventory.providers().len(), 0);

    nats_server.stop().await.context("failed to stop NATS")?;
    Ok(())
}
