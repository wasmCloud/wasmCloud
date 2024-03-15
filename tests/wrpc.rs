use core::str::{self, FromStr as _};
use core::time::Duration;

use std::collections::{BTreeSet, HashMap};
use std::net::Ipv4Addr;

use anyhow::{ensure, Context as _};
use nkeys::KeyPair;
use redis::AsyncCommands as _;
use serde::Deserialize;
use tempfile::tempdir;
use test_actors::{RUST_WRPC_PINGER_COMPONENT, RUST_WRPC_PONGER_COMPONENT_PREVIEW2};
use tokio::try_join;
use tracing_subscriber::prelude::*;
use url::Url;
use uuid::Uuid;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::provider::assert_start_provider;
use wasmcloud_test_util::{
    actor::assert_scale_actor, host::WasmCloudTestHost, lattice::link::assert_advertise_link,
};

pub mod common;
use common::free_port;
use common::nats::start_nats;
use common::redis::start_redis;

const LATTICE: &str = "default";
const PINGER_COMPONENT_ID: &str = "wrpc_pinger_component";
const PONGER_COMPONENT_ID: &str = "wrpc_ponger_component";

async fn assert_incoming_http(
    port: u16,
    redis_conn: &mut redis::aio::ConnectionManager,
) -> anyhow::Result<()> {
    let body = format!(
        r#"{{"min":42,"max":4242,"config_key":"test-config-data","authority":"localhost:{port}"}}"#,
    );

    redis::Cmd::set("foo", "bar")
        .query_async(redis_conn)
        .await
        .context("failed to set `foo` key in Redis")?;
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(20))
        .build()
        .context("failed to build HTTP client")?;
    let http_res = http_client
        .post(format!("http://localhost:{port}/foo?bar=baz"))
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
        .query_async(redis_conn)
        .await
        .context("failed to get `counter` key in Redis")?;
    ensure!(redis_res == redis::Value::Data(b"42".to_vec()));
    let redis_res: redis::Value = redis::Cmd::get("result")
        .query_async(redis_conn)
        .await
        .context("failed to get `result` key in Redis")?;
    ensure!(redis_res == redis::Value::Data(http_res.into()));
    Ok(())
}

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

    let ((nats_server, nats_url, nats_client), (redis_server, redis_url)) = try_join!(
        async { start_nats().await.context("failed to start NATS") },
        async { start_redis().await.context("failed to start Redis") }
    )?;
    let redis_client =
        redis::Client::open(redis_url.as_str()).context("failed to connect to Redis")?;
    let mut redis_conn = redis_client
        .get_tokio_connection_manager()
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

    let blobstore_fs_provider_key = KeyPair::from_seed(test_providers::RUST_BLOBSTORE_FS_SUBJECT)
        .context("failed to parse `rust-blobstore-fs` provider key")?;
    let blobstore_fs_provider_url = Url::from_file_path(test_providers::RUST_BLOBSTORE_FS)
        .expect("failed to construct provider ref");

    let httpclient_provider_key = KeyPair::from_seed(test_providers::RUST_HTTPCLIENT_SUBJECT)
        .context("failed to parse `rust-httpclient` provider key")?;
    let httpclient_provider_url = Url::from_file_path(test_providers::RUST_HTTPCLIENT)
        .expect("failed to construct provider ref");

    let httpserver_provider_key = KeyPair::from_seed(test_providers::RUST_HTTPSERVER_SUBJECT)
        .context("failed to parse `rust-httpserver` provider key")?;
    let httpserver_provider_url = Url::from_file_path(test_providers::RUST_HTTPSERVER)
        .expect("failed to construct provider ref");

    let kvredis_provider_key = KeyPair::from_seed(test_providers::RUST_KVREDIS_SUBJECT)
        .context("failed to parse `rust-kvredis` provider key")?;
    let kvredis_provider_url = Url::from_file_path(test_providers::RUST_KVREDIS)
        .expect("failed to construct provider ref");

    let blobstore_dir = tempdir()?;
    let http_port = free_port().await?;

    let blobstore_fs_config_name = "blobstore-fs-root".to_string();
    let http_config_name = "http-default-address".to_string();
    let kvredis_config_name = "kvredis-url".to_string();

    // Create configuration for the HTTP provider
    assert_config_put(
        &ctl_client,
        &http_config_name,
        HashMap::from_iter([(
            "ADDRESS".to_string(),
            format!("{}:{http_port}", Ipv4Addr::LOCALHOST),
        )]),
    )
    .await?;

    // Create configuration for the Redis provider
    assert_config_put(
        &ctl_client,
        &kvredis_config_name,
        HashMap::from_iter([("URL".to_string(), redis_url.to_string())]),
    )
    .await?;

    // Create configuration for the blobstore-fs provider
    assert_config_put(
        &ctl_client,
        &blobstore_fs_config_name,
        HashMap::from_iter([(
            "ROOT".to_string(),
            blobstore_dir.path().to_string_lossy().into(),
        )]),
    )
    .await?;

    assert_start_provider(wasmcloud_test_util::provider::StartProviderArgs {
        client: &ctl_client,
        lattice: LATTICE,
        host_key: &host.host_key(),
        provider_key: &blobstore_fs_provider_key,
        provider_id: &blobstore_fs_provider_key.public_key(),
        url: &blobstore_fs_provider_url,
        config: vec![],
    })
    .await?;

    assert_start_provider(wasmcloud_test_util::provider::StartProviderArgs {
        client: &ctl_client,
        lattice: LATTICE,
        host_key: &host.host_key(),
        provider_key: &httpserver_provider_key,
        provider_id: &httpserver_provider_key.public_key(),
        url: &httpserver_provider_url,
        config: vec![],
    })
    .await?;

    assert_start_provider(wasmcloud_test_util::provider::StartProviderArgs {
        client: &ctl_client,
        lattice: LATTICE,
        host_key: &host.host_key(),
        provider_key: &httpclient_provider_key,
        provider_id: &httpclient_provider_key.public_key(),
        url: &httpclient_provider_url,
        config: vec![],
    })
    .await?;

    assert_start_provider(wasmcloud_test_util::provider::StartProviderArgs {
        client: &ctl_client,
        lattice: LATTICE,
        host_key: &host.host_key(),
        provider_key: &kvredis_provider_key,
        provider_id: &kvredis_provider_key.public_key(),
        url: &kvredis_provider_url,
        config: vec![],
    })
    .await?;

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
    .expect("should've scaled pinger actor");

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
    .expect("should've scaled actor");

    assert_advertise_link(
        &ctl_client,
        httpserver_provider_key.public_key(),
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
        httpclient_provider_key.public_key(),
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
        kvredis_provider_key.public_key(),
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
        blobstore_fs_provider_key.public_key(),
        "default",
        "wasi",
        "blobstore",
        vec!["blobstore".to_string()],
        vec![],
        vec![blobstore_fs_config_name],
    )
    .await
    .context("failed to advertise link")?;

    assert_incoming_http(http_port, &mut redis_conn).await?;

    try_join!(
        async { nats_server.stop().await.context("failed to stop NATS") },
        async { redis_server.stop().await.context("failed to stop Redis") },
    )?;
    Ok(())
}
