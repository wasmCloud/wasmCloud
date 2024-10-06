#![cfg(all(feature = "provider-http-server", feature = "provider-keyvalue-redis",))]

use anyhow::Context as _;
use core::str;
use core::time::Duration;
use redis::Client;
use std::net::Ipv4Addr;
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
// NOTE : A component should be created implementing the `watcher` capability
// The component should have 2 functions as described in watch.wit
// Let the functions just perform a simple outgoing request with the key-value pair or just key (in case of delete action), this will help us to assert actions later on.
// on_set and on_delete should be called in a handling interface like `handle` or `handle_message`
// so that the key-value / key to watch for can be added via http requests through query params.
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

    client
        .get(url)
        .send()
        .await
        .context("failed to connect to server")?
        .text()
        .await
        .context("failed to get response text")
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

    let (rust_http_server, rust_keyvalue_redis) = join!(
        providers::rust_http_server(),
        providers::rust_keyvalue_redis(),
    );

    let rust_http_server_id = rust_http_server.subject.public_key();
    let rust_keyvalue_redis_id = rust_keyvalue_redis.subject.public_key();

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
                    &keyvalue_redis_config_name,
                    [("URL".to_string(), redis_url.to_string())],
                ),
            )
            .context("failed to put configuration")
        },
        async {
            let host_key = host.host_key();
            let rust_http_server_url = rust_http_server.url();
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
        vec![keyvalue_redis_config_name.clone()],
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
        vec![keyvalue_redis_config_name],
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
        assert_watch(&http_client, http_port, "on_delete", "foo", None).await?,
        "foo: Successfully created on_delete trigger"
    );

    // NOTE : Perform the set and/or delete operations on the respective key-value pair on the redis client.
    // And verify the watch capability : How ? : check if the log statement have been reflected in the terminal/output
    // Do : Set foo = bar wait 5 seconds , verify by checking logs and then delete bar in foo for redis and wait 5 seconds.
    let redis_client = Client::open(redis_url.as_str())?;
    let mut con = redis_client.get_connection()?;

    let _: () = redis::cmd("SET").arg("foo").arg("bar").query(&mut con)?;

    // Wait for set operation to be processed
    sleep(Duration::from_secs(5)).await;

    let _: () = redis::cmd("DEL").arg("foo").query(&mut con)?;

    // Wait for delete operation to be processed
    sleep(Duration::from_secs(5)).await;
    // Verification : Check logs of the component , i don't know how that's done.

    try_join!(
        async { nats_server.stop().await.context("failed to stop NATS") },
        async { redis_server.stop().await.context("failed to stop Redis") },
    )?;
    Ok(())
}
