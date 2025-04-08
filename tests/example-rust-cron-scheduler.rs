#![cfg(all(feature = "provider-cron-scheduler"))]
use core::str;
use core::time::Duration;

use anyhow::Context as _;
use futures::join;
use tokio::try_join;
use tracing_subscriber::prelude::*;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::provider::{assert_start_provider, StartProviderArgs};
use wasmcloud_test_util::{component::assert_scale_component, host::WasmCloudTestHost};

use test_components::RUST_CRON_SCHEDULER;

pub mod common;
use common::nats::start_nats;
use common::providers;

const LATTICE: &str = "default";
const COMPONENT_ID: &str = "cron_scheduler";

#[tokio::test]
async fn example_rust_cron_scheduler() -> anyhow::Result<()> {
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
        .context("failed to start NATS");

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client)
        .lattice(LATTICE.to_string())
        .build();
    // Build the host
    let host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let cron_config_name = "cron-config".to_string();

    // Loads the Key-Operation pairs to be watched
    let cron_config = "demo=0 0 12 * * ?:{'foo':'bar'}".to_string();

    let (rust_cron_scheduler, rust_http_client) = join!(
        providers::rust_cron_scheduler(),
        providers::rust_http_client(),
    );

    let rust_cron_scheduler_id = rust_cron_scheduler.subject.public_key();
    let rust_http_client_id = rust_http_client.subject.public_key();

    try_join!(
        async {
            try_join!(assert_config_put(
                &ctl_client,
                &cron_config_name,
                [("cronjobs".to_string(), cron_config)],
            ),)
            .context("failed to put configuration")
        },
        async {
            let host_key = host.host_key();
            let rust_http_client_url = rust_http_client.url();
            let rust_cron_scheduler_url = rust_cron_scheduler.url();
            let host_id = host_key.public_key();
            try_join!(
                assert_start_provider(StartProviderArgs {
                    client: &ctl_client,
                    host_id: &host_id,
                    provider_id: &rust_cron_scheduler_id,
                    provider_ref: rust_cron_scheduler_url.as_str(),
                    config: vec![],
                }),
                assert_start_provider(StartProviderArgs {
                    client: &ctl_client,
                    host_id: &host_id,
                    provider_id: &rust_http_client_id,
                    provider_ref: rust_http_client_url.as_str(),
                    config: vec![],
                }),
            )
            .context("failed to start providers")
        },
        async {
            assert_scale_component(
                &ctl_client,
                host.host_key().public_key(),
                format!("file://{RUST_CRON_SCHEDULER}"),
                COMPONENT_ID,
                None,
                5,
                Vec::new(),
                Duration::from_secs(10),
            )
            .await
            .context("failed to scale `rust-http-keyvalue-counter` component")
        }
    )?;

    // TODO(Aditya): Create link and configure validation logic

    nats_server.stop().await.context("failed to stop NATS")?;
    Ok(())
}
