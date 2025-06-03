#![cfg(feature = "wasmcloud")]

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, ensure, Context, Result};
use async_nats::jetstream;
use common::secrets::NatsKvSecretsBackend;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::TokioIo;
use secrets_nats_kv::PutSecretRequest;
use test_components::{
    RUST_PINGER_CONFIG_COMPONENT_PREVIEW2_SIGNED, RUST_PONGER_CONFIG_COMPONENT_PREVIEW2_SIGNED,
};
use tokio::net::TcpListener;
use tokio::try_join;
use tracing::info;
use tracing::instrument;
use tracing_subscriber::prelude::*;
use wasmcloud_host::wasmbus::config::BundleGenerator;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::lattice::config::assert_put_secret_reference;
use wasmcloud_test_util::{
    component::assert_scale_component, host::WasmCloudTestHost,
    lattice::link::assert_advertise_link,
};

pub mod common;
use common::{nats::start_nats, serve_incoming_http};

const LATTICE: &str = "config";
const PINGER_COMPONENT_ID: &str = "pinger_component";
const PONGER_COMPONENT_ID: &str = "ponger_component";

#[instrument(skip_all, ret)]
#[tokio::test(flavor = "multi_thread")]
async fn config_updates() -> Result<()> {
    let (nats_server, _, nats_client) = start_nats(None, true)
        .await
        .map(|res| (res.0, res.1, res.2.unwrap()))
        .context("failed to start backing services")?;

    let store = jetstream::new(nats_client)
        .create_key_value(jetstream::kv::Config {
            bucket: "CONFIG".into(),
            ..Default::default()
        })
        .await
        .context("Unable to set up NATS KV store for test")?;

    let generator = BundleGenerator::new(Arc::new(store.clone()));

    // First test that a non-existent config item returns an error
    generator
        .generate(vec!["inoexist".to_string()])
        .await
        .map(|_| ())
        .expect_err("Should have errored if a config didn't exist");

    // Now create some config items
    put_config(&store, "foo", [("star".to_string(), "wars".to_string())]).await?;
    put_config(
        &store,
        "bar",
        [("captain".to_string(), "picard".to_string())],
    )
    .await?;

    let bundle = generator
        .generate(vec!["foo".to_string(), "bar".to_string()])
        .await
        .expect("Should be able to generate config bundle");
    // Give it a sec to populate from the store
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        *bundle.get_config().await,
        HashMap::from([
            ("star".to_string(), "wars".to_string()),
            ("captain".to_string(), "picard".to_string())
        ])
    );

    // Update the config, give it a sec to update and then make sure it updated properly
    put_config(
        &store,
        "bar",
        [
            ("captain".to_string(), "picard".to_string()),
            ("star".to_string(), "trek".to_string()),
        ],
    )
    .await?;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        *bundle.get_config().await,
        HashMap::from([
            ("star".to_string(), "trek".to_string()),
            ("captain".to_string(), "picard".to_string())
        ])
    );

    // Generate a new bundle using the foo config and make sure it has the right data. This also should exercise the caching behavior, though it is hard to actually test it is pulling from the cache
    let bundle2 = generator
        .generate(vec!["foo".to_string()])
        .await
        .expect("Should be able to generate config bundle");
    assert_eq!(
        *bundle2.get_config().await,
        HashMap::from([("star".to_string(), "wars".to_string())])
    );

    let _ = nats_server.stop().await;
    Ok(())
}

#[instrument(skip_all, ret)]
async fn put_config(
    store: &jetstream::kv::Store,
    name: &str,
    config: impl Into<HashMap<String, String>>,
) -> Result<()> {
    let data = serde_json::to_vec(&config.into()).expect("Should be able to serialize config");
    store
        .put(name, data.into())
        .await
        .context("Failed to put config")
        .map(|_| ())
}

#[instrument(skip_all, ret)]
#[tokio::test]
async fn config_e2e() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().compact().without_time())
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,cranelift_codegen=warn,wasmcloud=trace")
            }),
        )
        .init();

    // Start NATS server
    let (nats_server, nats_url, nats_client) = start_nats(None, true)
        .await
        .map(|res| (res.0, res.1, res.2.unwrap()))
        .expect("should be able to start NATS");

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICE.to_string())
        .build();
    let wrpc_client = wrpc_transport_nats::Client::new(
        nats_client.clone(),
        format!("{LATTICE}.{PINGER_COMPONENT_ID}"),
        None,
    )
    .await?;
    let wrpc_client = Arc::new(wrpc_client);
    // Build the host
    let host = WasmCloudTestHost::start_custom(
        &nats_url,
        LATTICE,
        None,
        None,
        None,
        Some("wasmcloud.secrets".to_string()),
        None,
    )
    .await
    .context("failed to start test host")?;
    let host_id = host.host_key().public_key();

    let nats_kv_secrets_backend = NatsKvSecretsBackend::new(
        "wasmcloud.secrets".to_string(),
        "TEST_SECRET_default".to_string(),
        nats_url.to_string(),
    )
    .await?;

    nats_kv_secrets_backend.ensure_build().await?;
    let secrets_backend_server = nats_kv_secrets_backend.start().await?;
    nats_kv_secrets_backend
        .put_secret(PutSecretRequest {
            key: "ponger".to_string(),
            string_secret: Some("sup3rs3cr3t-v4lu3".to_string()),
            ..Default::default()
        })
        .await?;

    // Add mapping to allow the component to access the secrets
    use tokio::io::AsyncReadExt;
    let mut component_bytes = vec![];
    let mut component = tokio::fs::File::open(RUST_PONGER_CONFIG_COMPONENT_PREVIEW2_SIGNED).await?;
    let _ = component.read_to_end(&mut component_bytes).await?;
    let component_token =
        wascap::wasm::extract_claims(&component_bytes)?.expect("claims to be valid");
    let component_key = nkeys::KeyPair::from_public_key(&component_token.claims.subject)?;

    let mut secrets: std::collections::HashSet<String> = std::collections::HashSet::new();
    secrets.insert("ponger".to_string());

    nats_kv_secrets_backend
        .add_mapping(&component_key.public_key(), secrets)
        .await?;

    // Put configs for first component
    assert_config_put(
        &ctl_client,
        "pinger",
        [
            ("foo".to_string(), "bar".to_string()),
            ("test".to_string(), "yay".to_string()),
        ],
    )
    .await?;
    assert_config_put(
        &ctl_client,
        "pinger_override",
        [("foo".to_string(), "baz".to_string())],
    )
    .await?;
    // Scale pinger
    assert_scale_component(
        &ctl_client,
        &host_id,
        format!("file://{RUST_PINGER_CONFIG_COMPONENT_PREVIEW2_SIGNED}"),
        PINGER_COMPONENT_ID,
        None,
        5,
        vec!["pinger".to_string(), "pinger_override".to_string()],
        Duration::from_secs(10),
    )
    .await
    .expect("should've scaled pinger component");

    // Put config for second component
    assert_config_put(
        &ctl_client,
        "ponger",
        [("pong".to_string(), "config".to_string())],
    )
    .await?;
    // Put secret for second component
    assert_put_secret_reference(
        &ctl_client,
        "ponger",
        "ponger",
        "nats-kv",
        None,
        None,
        HashMap::with_capacity(0),
    )
    .await?;
    // Scale ponger
    assert_scale_component(
        &ctl_client,
        &host_id,
        format!("file://{RUST_PONGER_CONFIG_COMPONENT_PREVIEW2_SIGNED}"),
        PONGER_COMPONENT_ID,
        None,
        5,
        vec!["ponger".to_string(), "SECRET_ponger".to_string()],
        Duration::from_secs(10),
    )
    .await
    .expect("should've scaled component");

    // Link pinger --wrpc:testing/pingpong--> ponger
    assert_advertise_link(
        &ctl_client,
        PINGER_COMPONENT_ID,
        PONGER_COMPONENT_ID,
        "default",
        "test-components",
        "testing",
        vec!["pingpong".to_string()],
        vec![],
        vec![],
    )
    .await
    .expect("should advertise link");

    assert_incoming_http(&wrpc_client).await?;

    secrets_backend_server
        .stop()
        .await
        .expect("should be able to stop secrets backend");

    nats_server
        .stop()
        .await
        .expect("should be able to stop NATS");
    Ok(())
}

#[instrument(skip_all, ret)]
async fn assert_incoming_http(
    wrpc_client: &Arc<wrpc_transport_nats::Client>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")?;
    let addr = listener
        .local_addr()
        .context("failed to query listener local address")?;
    try_join!(
        async {
            info!("awaiting connection");
            let (stream, addr) = listener
                .accept()
                .await
                .context("failed to accept connection")?;
            info!("accepted connection from {addr}");
            hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection(
                    TokioIo::new(stream),
                    hyper::service::service_fn(move |request| {
                        let wrpc_client = Arc::clone(wrpc_client);
                        async move { serve_incoming_http(&wrpc_client, request).await }
                    }),
                )
                .await
                .map_err(|err| anyhow!(err).context("failed to serve connection"))
        },
        async {
            let body = r#"{"config_key":"foo"}"#;
            let http_client = reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .connect_timeout(Duration::from_secs(20))
                .build()
                .context("failed to build HTTP client")?;
            let http_res = http_client
                .post(format!("http://localhost:{}/", addr.port()))
                .header("test-header", "test-value")
                .body(body)
                .send()
                .await
                .context("failed to connect to server")?
                .text()
                .await
                .context("failed to get response text")?;
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Response {
                single_val: Option<String>,
                multi_val: HashMap<String, String>,
                pong: String,
                pong_secret: String,
            }
            let Response {
                single_val,
                multi_val,
                pong,
                pong_secret,
            } = serde_json::from_str(&http_res).context("failed to decode body as JSON")?;
            ensure!(pong == "config", "pong value was not correct");
            ensure!(
                pong_secret == "sup3rs3cr3t-v4lu3",
                "pong_secret value was not correct"
            );
            ensure!(
                single_val == Some("baz".to_string()),
                "single value was not correct"
            );
            ensure!(
                multi_val
                    == HashMap::from([
                        ("foo".to_string(), "baz".to_string()),
                        ("test".to_string(), "yay".to_string()),
                    ]),
                "multi value was not correct"
            );
            Ok(())
        }
    )?;
    Ok(())
}
