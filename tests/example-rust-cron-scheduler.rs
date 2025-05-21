#![cfg(all(feature = "provider-cron-scheduler", feature = "provider-http-client",))]
use core::str;
use core::time::Duration;

use anyhow::Context as _;
use futures::join;
use test_components::RUST_CRON_SCHEDULER;
use tokio::try_join;
use tracing::debug;
use tracing_subscriber::prelude::*;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::provider::{assert_start_provider, StartProviderArgs};
use wasmcloud_test_util::{
    component::assert_scale_component, host::WasmCloudTestHost,
    lattice::link::assert_advertise_link,
};

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
                tracing_subscriber::EnvFilter::new("debug,cranelift_codegen=warn,wasmcloud=trace")
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
    let host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let cron_config_name = "cron-config".to_string();

    // Loads the Key-Operation pairs to be watched
    let cron_config = r#"demo=*/3 * * * * ?:{"x1":"x2"}"#.to_string();

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
                [
                    ("cronjobs".to_string(), cron_config),
                    ("cluster_uris".to_string(), nats_url.as_str().to_string())
                ],
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

    // provider -> component for invoking exported functions from the bindings
    assert_advertise_link(
        &ctl_client,
        &rust_cron_scheduler_id,
        COMPONENT_ID,
        "default",
        "wasmcloud",
        "cron",
        vec!["scheduler".to_string()],
        vec![cron_config_name.clone()],
        vec![],
    )
    .await
    .context("failed to advertise link")?;

    // component -> Provider for calling outgoing requests

    assert_advertise_link(
        &ctl_client,
        COMPONENT_ID,
        &rust_http_client_id,
        "default",
        "wasi",
        "http",
        vec!["outgoing-handler".to_string()],
        vec![],
        vec![],
    )
    .await
    .context("failed to advertise link")?;

    let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter_clone = counter.clone();

    let addr = "127.0.0.1:3002";

    let app = axum::Router::new().route(
        "/payload",
        axum::routing::get(
            move |query: axum::extract::Query<std::collections::HashMap<String, String>>| {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

                    debug!("Received cron job request with params: {:?}", query.0);

                    assert!(
                        query.0.contains_key("x1") && query.0["x1"] == "x2",
                        "Expected x1=x2 in request params, got: {:?}",
                        query.0
                    );

                    "OK"
                }
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Mock HTTP Server Listening on {addr}");

    let server_handle = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service())
            .await
            .unwrap();
    });

    let test_duration_secs = 15;
    println!(
        "Waiting for {} seconds to collect cron job executions...",
        test_duration_secs
    );
    tokio::time::sleep(Duration::from_secs(test_duration_secs)).await;
    // Kill the server after 15 seconds
    server_handle.abort();

    let execution_count = counter.load(std::sync::atomic::Ordering::SeqCst);
    let min_expected = test_duration_secs / 3 - 2;
    let max_expected = test_duration_secs / 3 + 2;

    assert!(
        execution_count >= min_expected as usize && execution_count <= max_expected as usize,
        "Expected between {} and {} cron job executions, but got {}",
        min_expected,
        max_expected,
        execution_count
    );

    nats_server.stop().await.context("failed to stop NATS")?;
    Ok(())
}
