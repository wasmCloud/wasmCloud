#![cfg(feature = "providers")]

use core::str::{self, FromStr as _};
use core::time::Duration;

use std::collections::{BTreeSet, HashMap};
use std::net::Ipv4Addr;

use anyhow::{ensure, Context as _};
use redis::AsyncCommands as _;
use serde::Deserialize;
use tempfile::tempdir;
use test_actors::{RUST_WRPC_PINGER_COMPONENT, RUST_WRPC_PONGER_COMPONENT_PREVIEW2};
use tokio::time::sleep;
use tokio::{join, try_join};
use tracing_subscriber::prelude::*;
use uuid::Uuid;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::provider::assert_start_provider;
use wasmcloud_test_util::{
    actor::assert_scale_actor, host::WasmCloudTestHost, lattice::link::assert_advertise_link,
};

pub mod common;
use common::free_port;
use common::minio::start_minio;
use common::nats::start_nats;
use common::providers;
use common::redis::start_redis;

const LATTICE: &str = "default";
const PINGER_COMPONENT_ID: &str = "wrpc_pinger_component";
const PONGER_COMPONENT_ID: &str = "wrpc_ponger_component";

#[tokio::test]
async fn wrpc() -> anyhow::Result<()> {
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
    ) = try_join!(
        async {
            start_minio(minio_dir.path())
                .await
                .context("failed to start MinIO")
        },
        async { start_nats().await.context("failed to start NATS") },
        async { start_redis().await.context("failed to start Redis") }
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

    let blobstore_fs_config_name = "blobstore-fs-root".to_string();
    let blobstore_s3_config_name = "blobstore-s3".to_string();
    let http_config_name = "http-default-address".to_string();
    let kvredis_config_name = "kvredis-url".to_string();

    let (
        rust_blobstore_fs,
        rust_blobstore_s3,
        rust_http_client,
        rust_http_server,
        rust_keyvalue_redis,
    ) = join!(
        providers::rust_blobstore_fs(),
        providers::rust_blobstore_s3(),
        providers::rust_http_client(),
        providers::rust_http_server(),
        providers::rust_keyvalue_redis(),
    );

    let rust_blobstore_fs_id = rust_blobstore_fs.subject.public_key();
    let rust_blobstore_s3_id = rust_blobstore_s3.subject.public_key();
    let rust_http_client_id = rust_http_client.subject.public_key();
    let rust_http_server_id = rust_http_server.subject.public_key();
    let rust_keyvalue_redis_id = rust_keyvalue_redis.subject.public_key();

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
                    HashMap::from_iter([(
                        "ROOT".to_string(),
                        blobstore_fs_dir.path().to_string_lossy().into(),
                    )]),
                ),
                // Create configuration for the blobstore-s3 provider
                assert_config_put(
                    &ctl_client,
                    &blobstore_s3_config_name,
                    HashMap::from_iter([("config_json".to_string(), minio_config_json)]),
                ),
                // Create configuration for the HTTP provider
                assert_config_put(
                    &ctl_client,
                    &http_config_name,
                    HashMap::from_iter([(
                        "ADDRESS".to_string(),
                        format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
                    )]),
                ),
                // Create configuration for the Redis provider
                assert_config_put(
                    &ctl_client,
                    &kvredis_config_name,
                    HashMap::from_iter([("URL".to_string(), redis_url.to_string())]),
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
            try_join!(
                assert_start_provider(wasmcloud_test_util::provider::StartProviderArgs {
                    client: &ctl_client,
                    lattice: LATTICE,
                    host_key: &host_key,
                    provider_key: &rust_blobstore_fs.subject,
                    provider_id: &rust_blobstore_fs_id,
                    url: &rust_blobstore_fs_url,
                    config: vec![],
                }),
                assert_start_provider(wasmcloud_test_util::provider::StartProviderArgs {
                    client: &ctl_client,
                    lattice: LATTICE,
                    host_key: &host_key,
                    provider_key: &rust_blobstore_s3.subject,
                    provider_id: &rust_blobstore_s3_id,
                    url: &rust_blobstore_s3_url,
                    config: vec![],
                }),
                assert_start_provider(wasmcloud_test_util::provider::StartProviderArgs {
                    client: &ctl_client,
                    lattice: LATTICE,
                    host_key: &host_key,
                    provider_key: &rust_http_server.subject,
                    provider_id: &rust_http_server_id,
                    url: &rust_http_server_url,
                    config: vec![],
                }),
                assert_start_provider(wasmcloud_test_util::provider::StartProviderArgs {
                    client: &ctl_client,
                    lattice: LATTICE,
                    host_key: &host_key,
                    provider_key: &rust_http_client.subject,
                    provider_id: &rust_http_client_id,
                    url: &rust_http_client_url,
                    config: vec![],
                }),
                assert_start_provider(wasmcloud_test_util::provider::StartProviderArgs {
                    client: &ctl_client,
                    lattice: LATTICE,
                    host_key: &host_key,
                    provider_key: &rust_keyvalue_redis.subject,
                    provider_id: &rust_keyvalue_redis_id,
                    url: &rust_keyvalue_redis_url,
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
                        format!("file://{RUST_WRPC_PINGER_COMPONENT}"),
                        PINGER_COMPONENT_ID,
                        None,
                        5,
                        Vec::new(),
                    )
                    .await
                    .context("failed to scale `pinger` actor")
                },
                async {
                    // Scale ponger
                    assert_scale_actor(
                        &ctl_client,
                        &host.host_key(),
                        format!("file://{RUST_WRPC_PONGER_COMPONENT_PREVIEW2}"),
                        PONGER_COMPONENT_ID,
                        None,
                        5,
                        Vec::new(),
                    )
                    .await
                    .context("failed to scale `ponger` actor")
                },
            )
            .context("failed to scale actors")
        }
    )?;

    assert_advertise_link(
        &ctl_client,
        &rust_http_server_id,
        PINGER_COMPONENT_ID,
        "default",
        "wasi",
        "http",
        vec!["incoming-handler".to_string()],
        vec![http_config_name],
        vec![],
    )
    .await
    .context("failed to advertise link")?;

    // Link pinger --wrpc:testing/pingpong--> ponger
    assert_advertise_link(
        &ctl_client,
        PINGER_COMPONENT_ID,
        PONGER_COMPONENT_ID,
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
        PINGER_COMPONENT_ID,
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
        PINGER_COMPONENT_ID,
        &rust_keyvalue_redis_id,
        "default",
        "wasi",
        "keyvalue",
        vec!["atomic".to_string(), "eventual".to_string()],
        vec![],
        vec![kvredis_config_name],
    )
    .await
    .context("failed to advertise link")?;

    assert_advertise_link(
        &ctl_client,
        PINGER_COMPONENT_ID,
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
        PINGER_COMPONENT_ID,
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

    let body = format!(
        r#"{{"min":42,"max":4242,"config_key":"test-config-data","authority":"localhost:{http_port}"}}"#,
    );

    redis::Cmd::set("foo", "bar")
        .query_async(&mut redis_conn)
        .await
        .context("failed to set `foo` key in Redis")?;
    let http_client = reqwest::Client::builder()
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
    ensure!(redis_res == redis::Value::Data(http_res.into()));

    try_join!(
        async { minio_server.stop().await.context("failed to stop MinIO") },
        async { nats_server.stop().await.context("failed to stop NATS") },
        async { redis_server.stop().await.context("failed to stop Redis") },
    )?;
    Ok(())
}
