#![cfg(all(feature = "provider-keyvalue-redis", feature = "provider-http-client",))]

use anyhow::Context as _;
use core::str;
use core::time::Duration;
use redis::Client;
use tokio::time::sleep;
use tokio::{join, try_join};
use tracing::info;
use tracing_subscriber::prelude::*;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::provider::{assert_start_provider, StartProviderArgs};
use wasmcloud_test_util::{
    component::assert_scale_component, host::WasmCloudTestHost,
    lattice::link::assert_advertise_link,
};

use test_components::RUST_HTTP_KEYVALUE_WATCHER;

pub mod common;
use common::nats::start_nats;
use common::providers;
use common::redis::start_redis;

const LATTICE: &str = "default";
const COMPONENT_ID: &str = "http_keyvalue_watcher";

async fn start_alert_server() -> anyhow::Result<impl std::future::Future<Output = ()>> {
    let addr = "localhost:3001".to_string();
    info!("Starting alert server on {}", addr);

    let received_alerts = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let received_alerts_clone = received_alerts.clone();

    let app = axum::Router::new().route(
        "/alert",
        axum::routing::get(
            move |query: axum::extract::Query<std::collections::HashMap<String, String>>| {
                let received_alerts = received_alerts_clone.clone();
                async move {
                    for (key, value) in query.0 {
                        received_alerts
                            .lock()
                            .await
                            .push(format!("{}={}", key, value));
                    }
                    "OK"
                }
            },
        ),
    );

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let server = axum::serve(listener, app.into_make_service());

    tokio::spawn(async move { server.await.unwrap() });

    let shutdown_signal = async move {
        let alerts = received_alerts.lock().await;
        let expected_alerts = vec![
            "moo=jar".to_string(),
            "foo=mar".to_string(),
            "foo=nil".to_string(),
        ];

        for expected in expected_alerts {
            assert!(
                alerts.contains(&expected),
                "\nExpected alert: {}\nActual alerts: {:?}",
                expected,
                *alerts
            );
        }
    };

    Ok(shutdown_signal)
}

#[tokio::test]
async fn example_rust_keyvalue_watch() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    let ((nats_server, nats_url, nats_client), (redis_server, redis_url)) = try_join!(
        async {
            start_nats(None, true)
                .await
                .map(|res| (res.0, res.1, res.2.unwrap()))
                .context("failed to start NATS")
        },
        async { start_redis().await.context("failed to start Redis") },
    )?;
    // NOTE : Add a Http Provider for the component to interact with the lattice

    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client)
        .lattice(LATTICE.to_string())
        .build();

    let host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let keyvalue_redis_config_name = "keyvalue-redis".to_string();

    // Loads the Key-Operation pairs to be watched
    let keyvalue_watch_config = "SET@moo,SET@foo,DEL@foo".to_string();

    let (rust_keyvalue_redis, rust_http_client) = join!(
        providers::rust_keyvalue_redis(),
        providers::rust_http_client(),
    );

    let redis_client = Client::open(redis_url.as_str())?;
    let mut con = redis_client.get_connection()?;

    // Enable Keyspace notifications
    let _: () = redis::cmd("CONFIG")
        .arg("SET")
        .arg("notify-keyspace-events")
        .arg("K$g")
        .query(&mut con)?;

    sleep(Duration::from_secs(3)).await;

    let rust_keyvalue_redis_id = rust_keyvalue_redis.subject.public_key();
    let rust_http_client_id = rust_http_client.subject.public_key();

    try_join!(
        async {
            try_join!(assert_config_put(
                &ctl_client,
                &keyvalue_redis_config_name,
                [
                    ("URL".to_string(), redis_url.to_string()),
                    ("WATCH".to_string(), keyvalue_watch_config)
                ],
            ),)
            .context("failed to put configuration")
        },
        async {
            let host_key = host.host_key();
            let rust_http_client_url = rust_http_client.url();
            let rust_keyvalue_redis_url = rust_keyvalue_redis.url();
            let host_id = host_key.public_key();
            try_join!(
                assert_start_provider(StartProviderArgs {
                    client: &ctl_client,
                    host_id: &host_id,
                    provider_id: &rust_keyvalue_redis_id,
                    provider_ref: rust_keyvalue_redis_url.as_str(),
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
                format!("file://{RUST_HTTP_KEYVALUE_WATCHER}"),
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
        &rust_keyvalue_redis_id,
        COMPONENT_ID,
        "default",
        "wasi",
        "keyvalue",
        vec!["watcher".to_string()],
        vec![keyvalue_redis_config_name.clone()],
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

    // Wait for data to be propagated across lattice
    sleep(Duration::from_secs(1)).await;

    let shutdown_signal = start_alert_server().await?;

    let _: () = redis::cmd("SET").arg("moo").arg("jar").query(&mut con)?;
    let _: () = redis::cmd("SET").arg("foo").arg("mar").query(&mut con)?;
    sleep(Duration::from_secs(2)).await;
    let _: () = redis::cmd("DEL").arg("foo").query(&mut con)?;

    sleep(Duration::from_secs(5)).await;

    shutdown_signal.await;

    try_join!(
        async { nats_server.stop().await.context("failed to stop NATS") },
        async { redis_server.stop().await.context("failed to stop Redis") },
    )?;
    Ok(())
}
