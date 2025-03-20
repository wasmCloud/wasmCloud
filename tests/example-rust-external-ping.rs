use anyhow::Context as _;
use common::providers;
use core::time::Duration;
use std::net::Ipv4Addr;
use test_components::RUST_PINGER_EXTERNAL_COMPONENT;
use tokio::time::sleep;
use tokio::try_join;
use tracing::{info, warn};
use tracing_subscriber::prelude::*;
use wasmcloud_core::tls::NativeRootsExt as _;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::provider::{assert_start_provider, StartProviderArgs};
use wasmcloud_test_util::{
    component::assert_scale_component, host::WasmCloudTestHost,
    lattice::link::assert_advertise_link,
};

mod common;
use common::free_port;
use common::nats::start_nats;
use common::wrpc::start_wrpc;

const LATTICE: &str = "default";
const COMPONENT_ID: &str = "pinger";

#[tokio::test]
async fn example_rust_external_ping() -> anyhow::Result<()> {
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
    info!("NATS URL: {}", nats_url);

    let external_service = start_wrpc(nats_url.as_str())
        .await
        .context("failed to start external wRPC service")?;

    let host = WasmCloudTestHost::start(nats_url.as_str(), LATTICE)
        .await
        .context("failed to start test host")?;

    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICE.to_string())
        .build();

    let http_port = free_port().await?;
    let http_server_config_name = "http-server".to_string();
    let http_server_id = "http-server";
    let host_id = host.host_key().public_key();

    try_join!(
        async {
            assert_config_put(
                &ctl_client,
                &http_server_config_name,
                [(
                    "ADDRESS".to_string(),
                    format!("{}:{}", Ipv4Addr::LOCALHOST, http_port),
                )],
            )
            .await
            .context("failed to put configuration")
        },
        async {
            assert_start_provider(StartProviderArgs {
                client: &ctl_client,
                host_id: &host_id,
                provider_id: http_server_id,
                provider_ref: providers::builtin_http_server().as_str(),
                config: vec![],
            })
            .await
            .context("failed to start HTTP provider")
        },
        async {
            assert_scale_component(
                &ctl_client,
                &host_id,
                format!("file://{RUST_PINGER_EXTERNAL_COMPONENT}"),
                COMPONENT_ID,
                None,
                5,
                Vec::new(),
                Duration::from_secs(10),
            )
            .await
            .context("failed to scale pinger component")
        }
    )?;

    assert_advertise_link(
        &ctl_client,
        http_server_id,
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

    sleep(Duration::from_secs(2)).await;

    let url = format!("http://localhost:{}/", http_port);
    let res = http_client
        .get(&url)
        .send()
        .await
        .context("failed to send HTTP request")?;
    let text = res.text().await.context("failed to get response text")?;
    info!("Response: {}", text);

    assert_eq!(text, "External ping successful!");

    nats_server.stop().await.context("failed to stop NATS")?;
    external_service
        .stop()
        .await
        .context("failed to stop external service")?;

    Ok(())
}
