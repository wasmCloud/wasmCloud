#![cfg(feature = "provider-http-server")]

use core::str;
use core::time::Duration;

use std::net::Ipv4Addr;

use anyhow::Context as _;
use tokio::time::sleep;
use tokio::{join, try_join};
use tracing_subscriber::prelude::*;
use wasmcloud_core::tls::NativeRootsExt as _;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::provider::{assert_start_provider, StartProviderArgs};
use wasmcloud_test_util::{
    component::assert_scale_component, host::WasmCloudTestHost,
    lattice::link::assert_advertise_link,
};

pub mod common;
use common::free_port;
use common::nats::start_nats;
use common::providers;

use test_components::RUST_HTTP_CALCULATOR;

const LATTICE: &str = "default";
const COMPONENT_ID: &str = "http_calculator";

async fn assert_increment(
    client: &reqwest::Client,
    port: u16,
    path: &str,
) -> anyhow::Result<String> {
    client
        .get(format!("http://localhost:{port}{path}"))
        .send()
        .await
        .context("failed to connect to server")?
        .text()
        .await
        .context("failed to get response text")
}

#[tokio::test]
async fn example_rust_http_calculator() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    let (nats_server, nats_url, nats_client) = start_nats(None, true)
        .await
        .map(|res| (res.0, res.1, res.2.unwrap()))
        .context("failed to start NATS")?;

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client)
        .lattice(LATTICE.to_string())
        .build();
    // Build the host
    let host = WasmCloudTestHost::start_v2_providers(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let http_port = free_port().await?;

    let http_server_config_name = "http-server".to_string();
    let calculator_config_name = "calculator".to_string();

    let (rust_http_server, rust_calculator) = join!(
        providers::rust_http_server(),
        providers::rust_calculator_example(),
    );

    let rust_http_server_id = rust_http_server.subject.public_key();
    let rust_calculator_id = rust_calculator.subject.public_key();

    try_join!(
        async {
            try_join!(
                assert_config_put(
                    &ctl_client,
                    &http_server_config_name,
                    [(
                        "ADDRESS".to_string(),
                        format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
                    )],
                ),
                assert_config_put(
                    &ctl_client,
                    &calculator_config_name,
                    [
                        ("enable_addition".to_string(), "true".to_string()),
                        ("enable_subtraction".to_string(), "true".to_string())
                    ],
                ),
            )
            .context("failed to put configuration")
        },
        async {
            let host_key = host.host_key();
            let rust_http_server_url = rust_http_server.url();
            let rust_calculator_url = rust_calculator.url();
            let host_id = host_key.public_key();
            try_join!(
                assert_start_provider(StartProviderArgs {
                    client: &ctl_client,
                    host_id: &host_id,
                    provider_id: &rust_http_server_id,
                    provider_ref: rust_http_server_url.as_str(),
                    config: vec![],
                }),
                assert_start_provider(StartProviderArgs {
                    client: &ctl_client,
                    host_id: &host_id,
                    provider_id: &rust_calculator_id,
                    provider_ref: rust_calculator_url.as_str(),
                    config: vec![],
                }),
            )
            .context("failed to start providers")
        },
        async {
            assert_scale_component(
                &ctl_client,
                host.host_key().public_key(),
                format!("file://{RUST_HTTP_CALCULATOR}"),
                COMPONENT_ID,
                None,
                5,
                Vec::new(),
                Duration::from_secs(10),
            )
            .await
            .context("failed to scale `rust-http-calculator` component")
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

    assert_advertise_link(
        &ctl_client,
        COMPONENT_ID,
        &rust_calculator_id,
        "default",
        "wasmcloud",
        "calculator",
        vec!["calculator".to_string()],
        vec![],
        vec![calculator_config_name],
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

    assert_eq!(
        assert_increment(&http_client, http_port, "").await?,
        "Counter /: 1\n"
    );

    assert_eq!(
        assert_increment(&http_client, http_port, "/").await?,
        "Counter /: 2\n"
    );

    assert_eq!(
        assert_increment(&http_client, http_port, "/test/path").await?,
        "Counter /test/path: 1\n"
    );

    assert_eq!(
        assert_increment(&http_client, http_port, "/foo").await?,
        "Counter /foo: 1\n"
    );

    assert_eq!(
        assert_increment(&http_client, http_port, "/").await?,
        "Counter /: 3\n"
    );

    nats_server.stop().await.context("failed to stop NATS")?;
    Ok(())
}
