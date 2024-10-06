#![cfg(all(
    feature = "provider-http-server",
    feature = "provider-keyvalue-redis",
    feature = "provider-http-client",
))]

use anyhow::Context as _;
use core::str;
use core::time::Duration;
use redis::Client;
use std::net::Ipv4Addr;
use tokio::time::sleep;
use tokio::{join, try_join};
use tracing::info;
use tracing_subscriber::prelude::*;
use wasmcloud_core::tls::NativeRootsExt as _;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::provider::{assert_start_provider, StartProviderArgs};
use wasmcloud_test_util::{
    component::assert_scale_component, host::WasmCloudTestHost,
    lattice::link::assert_advertise_link,
};

use test_components::RUST_HTTP_KEYVALUE_WATCHER;

pub mod common;
use common::free_port;
use common::nats::start_nats;
use common::providers;
use common::redis::start_redis;

const LATTICE: &str = "default";
const COMPONENT_ID: &str = "http_keyvalue_watcher";

async fn assert_watch(
    client: &reqwest::Client,
    port: u16,
    action: &str,
    key: &str,
    value: Option<&str>,
) -> anyhow::Result<String> {
    let url = match value {
        Some(v) => format!("http://localhost:{port}/action={action}&key={key}&value={v}"),
        None => format!("http://localhost:{port}/action={action}&key={key}"),
    };

    let res = client
        .get(url)
        .send()
        .await
        .context("failed to connect to server")?
        .text()
        .await
        .context("failed to get response text")?;
    println!("{}", res);

    sleep(Duration::from_secs(5)).await;

    Ok(res)
}

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
        let expected_alerts = vec!["moo=jar".to_string(), "foo=nil".to_string()];

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
        async { start_nats().await.context("failed to start NATS") },
        async { start_redis().await.context("failed to start Redis") },
    )?;
    // NOTE : Add a Http Provider for the component to interact with the lattice

    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client)
        .lattice(LATTICE.to_string())
        .build();

    let host = WasmCloudTestHost::start(&nats_url, LATTICE)
        .await
        .context("failed to start test host")?;

    let http_port = free_port().await?;

    let http_server_config_name = "http-server".to_string();

    let keyvalue_redis_config_name = "keyvalue-redis".to_string();

    let (rust_http_server, rust_keyvalue_redis, rust_http_client) = join!(
        providers::rust_http_server(),
        providers::rust_keyvalue_redis(),
        providers::rust_http_client(),
    );

    let rust_http_server_id = rust_http_server.subject.public_key();
    let rust_keyvalue_redis_id = rust_keyvalue_redis.subject.public_key();
    let rust_http_client_id = rust_http_client.subject.public_key();

    try_join!(
        async {
            try_join!(
                assert_config_put(
                    &ctl_client,
                    &keyvalue_redis_config_name,
                    [("URL".to_string(), redis_url.to_string())],
                ),
                assert_config_put(
                    &ctl_client,
                    &http_server_config_name,
                    [(
                        "ADDRESS".to_string(),
                        format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
                    )],
                ),
            )
            .context("failed to put configuration")
        },
        async {
            let host_key = host.host_key();
            let rust_http_server_url = rust_http_server.url();
            let rust_http_client_url = rust_http_client.url();
            let rust_keyvalue_redis_url = rust_keyvalue_redis.url();
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

    // component -> provider for adding k-v pairs for watching
    assert_advertise_link(
        &ctl_client,
        COMPONENT_ID,
        &rust_keyvalue_redis_id,
        "default",
        "wasi",
        "keyvalue",
        vec!["watcher".to_string(), "store".to_string()],
        vec![],
        vec![keyvalue_redis_config_name],
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

    let http_client = reqwest::Client::builder()
        .with_native_certificates()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .context("failed to build HTTP client")?;

    // Wait for data to be propagated across lattice
    sleep(Duration::from_secs(1)).await;

    assert_eq!(
        assert_watch(&http_client, http_port, "on_set", "foo", Some("bar")).await?,
        "foo: Successfully created on_set trigger"
    );

    assert_eq!(
        assert_watch(&http_client, http_port, "on_set", "moo", Some("jar")).await?,
        "moo: Successfully created on_set trigger"
    );

    assert_eq!(
        assert_watch(&http_client, http_port, "on_delete", "foo", None).await?,
        "foo: Successfully created on_delete trigger"
    );

    let shutdown_signal = start_alert_server().await?;

    let redis_client = Client::open(redis_url.as_str())?;
    let mut con = redis_client.get_connection()?;

    let _: () = redis::cmd("SET").arg("moo").arg("jar").query(&mut con)?;
    let _: () = redis::cmd("SET").arg("foo").arg("mar").query(&mut con)?;
    let _: () = redis::cmd("DEL").arg("foo").query(&mut con)?;

    sleep(Duration::from_secs(1)).await;

    shutdown_signal.await;

    try_join!(
        async { nats_server.stop().await.context("failed to stop NATS") },
        async { redis_server.stop().await.context("failed to stop Redis") },
    )?;
    Ok(())
}
