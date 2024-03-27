#![cfg(feature = "providers")]

use core::str::{self, FromStr as _};
use core::time::Duration;

use std::collections::{BTreeSet, HashMap};
use std::net::Ipv4Addr;

use anyhow::{ensure, Context as _};
use base64::Engine as _;
use redis::AsyncCommands as _;
use serde::Deserialize;
use serde_json::json;
use tempfile::tempdir;
use test_actors::{RUST_INTERFACES_HANDLER_REACTOR_PREVIEW2, RUST_INTERFACES_REACTOR};
use tokio::time::sleep;
use tokio::{join, try_join};
use tracing_subscriber::prelude::*;
use uuid::Uuid;
use wasmcloud_core::tls::NativeRootsExt as _;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::provider::{assert_start_provider, StartProviderArgs};
use wasmcloud_test_util::{
    actor::assert_scale_actor, host::WasmCloudTestHost, lattice::link::assert_advertise_link,
};

pub mod common;
use common::free_port;
use common::minio::start_minio;
use common::nats::start_nats;
use common::providers;
use common::redis::start_redis;
use common::vault::start_vault;

const LATTICE: &str = "default";
const INTERFACES_REACTOR_ID: &str = "interfaces_reactor";
const INTERFACES_HANDLER_REACTOR_ID: &str = "interfaces_handler_reactor";

#[tokio::test]
async fn interfaces() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    let blobstore_fs_dir = tempdir()?;
    let minio_dir = tempdir().context("failed to create a temporary directory")?;

    let (
        (minio_server, minio_url),
        (nats_server, nats_url, nats_client),
        (redis_server, redis_url),
        (vault_server, vault_url, vault_client),
    ) = try_join!(
        async {
            start_minio(minio_dir.path())
                .await
                .context("failed to start MinIO")
        },
        async { start_nats().await.context("failed to start NATS") },
        async { start_redis().await.context("failed to start Redis") },
        async { start_vault("test").await.context("failed to start Vault") },
    )?;
    let redis_client =
        redis::Client::open(redis_url.as_str()).context("failed to connect to Redis")?;
    let mut redis_conn = redis_client
        .get_connection_manager()
        .await
        .context("failed to construct Redis connection manager")?;

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICE.to_string())
        .build();
    // Build the host
    let host = WasmCloudTestHost::start(&nats_url, LATTICE, None, None)
        .await
        .context("failed to start test host")?;

    let http_port = free_port().await?;

    let blobstore_fs_config_name = "blobstore-fs".to_string();
    let blobstore_s3_config_name = "blobstore-s3".to_string();
    let http_server_config_name = "http-server".to_string();
    let keyvalue_redis_config_name = "keyvalue-redis".to_string();
    let keyvalue_vault_config_name = "keyvalue-vault".to_string();
    let messaging_nats_default_config_name = "messaging-nats".to_string();
    let messaging_nats_handler_config_name =
        "messaging-nats-interfaces-handler-reactor".to_string();
    let messaging_nats_interfaces_config_name = "messaging-nats-interfaces-reactor".to_string();

    let (
        rust_blobstore_fs,
        rust_blobstore_s3,
        rust_http_client,
        rust_http_server,
        rust_keyvalue_redis,
        rust_keyvalue_vault,
        rust_messaging_nats,
    ) = join!(
        providers::rust_blobstore_fs(),
        providers::rust_blobstore_s3(),
        providers::rust_http_client(),
        providers::rust_http_server(),
        providers::rust_keyvalue_redis(),
        providers::rust_keyvalue_vault(),
        providers::rust_messaging_nats(),
    );

    let rust_blobstore_fs_id = rust_blobstore_fs.subject.public_key();
    let rust_blobstore_s3_id = rust_blobstore_s3.subject.public_key();
    let rust_http_client_id = rust_http_client.subject.public_key();
    let rust_http_server_id = rust_http_server.subject.public_key();
    let rust_keyvalue_redis_id = rust_keyvalue_redis.subject.public_key();
    let rust_keyvalue_vault_id = rust_keyvalue_vault.subject.public_key();
    let rust_messaging_nats_id = rust_messaging_nats.subject.public_key();

    try_join!(
        async {
            let minio_config_json = format!(
                r#"{{"endpoint":"{minio_url}","access_key_id":"minioadmin","secret_access_key":"minioadmin","region":"us-east-1"}}"#
            );
            try_join!(
                // Create configuration for the blobstore-fs provider
                assert_config_put(
                    &ctl_client,
                    &blobstore_fs_config_name,
                    [(
                        "ROOT".to_string(),
                        blobstore_fs_dir.path().to_string_lossy().into(),
                    )],
                ),
                // Create configuration for the blobstore-s3 provider
                assert_config_put(
                    &ctl_client,
                    &blobstore_s3_config_name,
                    [("config_json".to_string(), minio_config_json)],
                ),
                // Create configuration for the HTTP provider
                assert_config_put(
                    &ctl_client,
                    &http_server_config_name,
                    [(
                        "ADDRESS".to_string(),
                        format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
                    )],
                ),
                // Create configuration for the Redis keyvalue provider
                assert_config_put(
                    &ctl_client,
                    &keyvalue_redis_config_name,
                    [("URL".to_string(), redis_url.to_string())],
                ),
                // Create configuration for the Vault keyvalue provider
                assert_config_put(
                    &ctl_client,
                    &keyvalue_vault_config_name,
                    [
                        ("ADDR".to_string(), vault_url.to_string()),
                        ("TOKEN".to_string(), "test".to_string()),
                    ],
                ),
                assert_config_put(
                    &ctl_client,
                    &messaging_nats_default_config_name,
                    [("cluster_uris".to_string(), nats_url.to_string()),],
                ),
                assert_config_put(
                    &ctl_client,
                    &messaging_nats_interfaces_config_name,
                    [("subscriptions".to_string(), "interfaces".to_string())],
                ),
                assert_config_put(
                    &ctl_client,
                    &messaging_nats_handler_config_name,
                    [(
                        "subscriptions".to_string(),
                        "interfaces-handler".to_string()
                    )],
                ),
            )
            .context("failed to put configuration")
        },
        async {
            let host_key = host.host_key();
            let rust_blobstore_fs_url = rust_blobstore_fs.url();
            let rust_blobstore_s3_url = rust_blobstore_s3.url();
            let rust_http_client_url = rust_http_client.url();
            let rust_http_server_url = rust_http_server.url();
            let rust_keyvalue_redis_url = rust_keyvalue_redis.url();
            let rust_keyvalue_vault_url = rust_keyvalue_vault.url();
            let rust_messaging_nats_url = rust_messaging_nats.url();
            try_join!(
                assert_start_provider(StartProviderArgs {
                    client: &ctl_client,
                    lattice: LATTICE,
                    host_key: &host_key,
                    provider_key: &rust_blobstore_fs.subject,
                    provider_id: &rust_blobstore_fs_id,
                    url: &rust_blobstore_fs_url,
                    config: vec![],
                }),
                assert_start_provider(StartProviderArgs {
                    client: &ctl_client,
                    lattice: LATTICE,
                    host_key: &host_key,
                    provider_key: &rust_blobstore_s3.subject,
                    provider_id: &rust_blobstore_s3_id,
                    url: &rust_blobstore_s3_url,
                    config: vec![],
                }),
                assert_start_provider(StartProviderArgs {
                    client: &ctl_client,
                    lattice: LATTICE,
                    host_key: &host_key,
                    provider_key: &rust_http_client.subject,
                    provider_id: &rust_http_client_id,
                    url: &rust_http_client_url,
                    config: vec![],
                }),
                assert_start_provider(StartProviderArgs {
                    client: &ctl_client,
                    lattice: LATTICE,
                    host_key: &host_key,
                    provider_key: &rust_http_server.subject,
                    provider_id: &rust_http_server_id,
                    url: &rust_http_server_url,
                    config: vec![],
                }),
                assert_start_provider(StartProviderArgs {
                    client: &ctl_client,
                    lattice: LATTICE,
                    host_key: &host_key,
                    provider_key: &rust_keyvalue_redis.subject,
                    provider_id: &rust_keyvalue_redis_id,
                    url: &rust_keyvalue_redis_url,
                    config: vec![],
                }),
                assert_start_provider(StartProviderArgs {
                    client: &ctl_client,
                    lattice: LATTICE,
                    host_key: &host_key,
                    provider_key: &rust_keyvalue_vault.subject,
                    provider_id: &rust_keyvalue_vault_id,
                    url: &rust_keyvalue_vault_url,
                    config: vec![],
                }),
                assert_start_provider(StartProviderArgs {
                    client: &ctl_client,
                    lattice: LATTICE,
                    host_key: &host_key,
                    provider_key: &rust_messaging_nats.subject,
                    provider_id: &rust_messaging_nats_id,
                    url: &rust_messaging_nats_url,
                    config: vec![],
                }),
            )
            .context("failed to start providers")
        },
        async {
            try_join!(
                async {
                    // Scale pinger
                    assert_scale_actor(
                        &ctl_client,
                        &host.host_key(),
                        format!("file://{RUST_INTERFACES_REACTOR}"),
                        INTERFACES_REACTOR_ID,
                        None,
                        5,
                        Vec::new(),
                    )
                    .await
                    .context("failed to scale `interface_reactor` actor")
                },
                async {
                    // Scale ponger
                    assert_scale_actor(
                        &ctl_client,
                        &host.host_key(),
                        format!("file://{RUST_INTERFACES_HANDLER_REACTOR_PREVIEW2}"),
                        INTERFACES_HANDLER_REACTOR_ID,
                        None,
                        5,
                        Vec::new(),
                    )
                    .await
                    .context("failed to scale `interface_handler_reactor` actor")
                },
            )
            .context("failed to scale actors")
        }
    )?;

    assert_advertise_link(
        &ctl_client,
        &rust_http_server_id,
        INTERFACES_REACTOR_ID,
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
        INTERFACES_REACTOR_ID,
        INTERFACES_HANDLER_REACTOR_ID,
        "default",
        "test-actors",
        "testing",
        vec!["pingpong".to_string(), "busybox".to_string()],
        vec![],
        vec![],
    )
    .await
    .context("failed to advertise link")?;

    assert_advertise_link(
        &ctl_client,
        INTERFACES_REACTOR_ID,
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

    assert_advertise_link(
        &ctl_client,
        INTERFACES_REACTOR_ID,
        &rust_keyvalue_redis_id,
        "default",
        "wasi",
        "keyvalue",
        vec!["atomic".to_string(), "eventual".to_string()],
        vec![],
        vec![keyvalue_redis_config_name],
    )
    .await
    .context("failed to advertise link")?;

    assert_advertise_link(
        &ctl_client,
        INTERFACES_REACTOR_ID,
        &rust_keyvalue_vault_id,
        "vault",
        "wasi",
        "keyvalue",
        vec!["eventual".to_string()],
        vec![],
        vec![keyvalue_vault_config_name],
    )
    .await
    .context("failed to advertise link")?;

    assert_advertise_link(
        &ctl_client,
        INTERFACES_REACTOR_ID,
        &rust_blobstore_fs_id,
        "default",
        "wasi",
        "blobstore",
        vec!["blobstore".to_string()],
        vec![],
        vec![blobstore_fs_config_name],
    )
    .await
    .context("failed to advertise link")?;

    assert_advertise_link(
        &ctl_client,
        INTERFACES_REACTOR_ID,
        &rust_blobstore_s3_id,
        "s3",
        "wasi",
        "blobstore",
        vec!["blobstore".to_string()],
        vec![],
        vec![blobstore_s3_config_name],
    )
    .await
    .context("failed to advertise link")?;

    assert_advertise_link(
        &ctl_client,
        INTERFACES_REACTOR_ID,
        &rust_messaging_nats_id,
        "default",
        "wasmcloud",
        "messaging",
        vec!["consumer".to_string()],
        vec![],
        vec![
            messaging_nats_default_config_name.clone(),
            messaging_nats_interfaces_config_name,
        ],
    )
    .await
    .context("failed to advertise link")?;

    assert_advertise_link(
        &ctl_client,
        INTERFACES_HANDLER_REACTOR_ID,
        &rust_messaging_nats_id,
        "default",
        "wasmcloud",
        "messaging",
        vec!["consumer".to_string()],
        vec![],
        vec![
            messaging_nats_default_config_name,
            messaging_nats_handler_config_name,
        ],
    )
    .await
    .context("failed to advertise link")?;

    let body = format!(
        r#"{{"min":42,"max":4242,"config_key":"test-config-data","authority":"localhost:{http_port}"}}"#,
    );
    redis::Cmd::set("foo", "bar")
        .query_async(&mut redis_conn)
        .await
        .context("failed to set `foo` key in Redis")?;
    vaultrs::kv2::set(
        &vault_client,
        "secret",
        "test",
        &json!({"foo": base64::engine::general_purpose::STANDARD_NO_PAD.encode(b"bar")}),
    )
    .await
    .context("failed to set `foo` key in Vault")?;

    let http_client = reqwest::Client::builder()
        .with_native_certificates()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .context("failed to build HTTP client")?;

    // Wait for data to be propagated across lattice
    sleep(Duration::from_secs(1)).await;

    let http_res = http_client
        .post(format!("http://localhost:{http_port}/foo?bar=baz"))
        .header("test-header", "test-value")
        .body(body)
        .send()
        .await
        .context("failed to connect to server")?
        .text()
        .await
        .context("failed to get response text")?;
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    // NOTE: If values are truly random, we have nothing to assert for some of these fields
    struct Response {
        #[allow(dead_code)]
        get_random_bytes: [u8; 8],
        #[allow(dead_code)]
        get_random_u64: u64,
        guid: String,
        random_in_range: u32,
        #[allow(dead_code)]
        random_32: u32,
        #[allow(dead_code)]
        long_value: String,
        config_value: Option<Vec<u8>>,
        all_config: Vec<(String, Vec<u8>)>,
        ping: String,
        meaning_of_universe: u8,
        split: Vec<String>,
        is_same: bool,
        archie: bool,
    }
    let Response {
        get_random_bytes: _,
        get_random_u64: _,
        guid,
        random_32: _,
        random_in_range,
        long_value,
        config_value,
        all_config,
        ping,
        meaning_of_universe,
        split,
        is_same,
        archie,
    } = serde_json::from_str(&http_res).context("failed to decode body as JSON")?;
    ensure!(Uuid::from_str(&guid).is_ok());
    ensure!(
        (42..=4242).contains(&random_in_range),
        "{random_in_range} should have been within range from 42 to 4242 inclusive"
    );
    ensure!(config_value.is_none());
    ensure!(all_config == []);
    ensure!(ping == "pong");
    ensure!(long_value == "1234567890".repeat(5000));
    ensure!(meaning_of_universe == 42);
    ensure!(split == ["hi", "there", "friend"]);
    ensure!(is_same);
    ensure!(archie);

    let redis_keys: BTreeSet<String> = redis_conn
        .keys("*")
        .await
        .context("failed to list keys in Redis")?;
    let expected_redis_keys = BTreeSet::from(["counter".into(), "result".into()]);
    ensure!(
        redis_keys == expected_redis_keys,
        r#"invalid keys in Redis:
  got: {redis_keys:?}
  expected: {expected_redis_keys:?}"#
    );

    let redis_res: redis::Value = redis::Cmd::get("counter")
        .query_async(&mut redis_conn)
        .await
        .context("failed to get `counter` key in Redis")?;
    ensure!(redis_res == redis::Value::Data(b"42".to_vec()));
    let redis_res: redis::Value = redis::Cmd::get("result")
        .query_async(&mut redis_conn)
        .await
        .context("failed to get `result` key in Redis")?;
    ensure!(redis_res == redis::Value::Data(http_res.clone().into()));

    let vault_res = vaultrs::kv2::read::<HashMap<String, String>>(&vault_client, "secret", "test")
        .await
        .context("failed to list vault keys")?;
    assert_eq!(
        vault_res,
        HashMap::from([(
            "result".to_string(),
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(http_res)
        )])
    );

    try_join!(
        async { minio_server.stop().await.context("failed to stop MinIO") },
        async { nats_server.stop().await.context("failed to stop NATS") },
        async { redis_server.stop().await.context("failed to stop Redis") },
        async { vault_server.stop().await.context("failed to stop Vault") },
    )?;
    Ok(())
}
