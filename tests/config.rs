use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, ensure, Context, Result};
use async_nats::jetstream;
use async_nats::HeaderMap;
use hyper_util::rt::TokioExecutor;
use hyper_util::rt::TokioIo;
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
    let (nats_server, _, nats_client, _) = start_nats()
        .await
        .context("failed to start backing services")?;

    let store = jetstream::new(nats_client)
        .create_key_value(jetstream::kv::Config {
            bucket: "CONFIG".into(),
            ..Default::default()
        })
        .await
        .context("Unable to set up NATS KV store for test")?;

    let generator = BundleGenerator::new(store.clone());

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
    let (nats_server, nats_url, nats_client, nats_client_0_33) =
        start_nats().await.expect("should be able to start NATS");

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client_0_33)
        .lattice(LATTICE.to_string())
        .build();
    let wrpc_client = wrpc_transport_nats::Client::new(
        nats_client.clone(),
        format!("{LATTICE}.{PINGER_COMPONENT_ID}"),
        None,
    );
    let wrpc_client = Arc::new(wrpc_client);
    // Build the host
    let host = WasmCloudTestHost::start_custom(
        &nats_url,
        LATTICE,
        None,
        None,
        None,
        Some("wasmcloud.secrets".to_string()),
    )
    .await
    .context("failed to start test host")?;

    // TODO: abstract to function
    std::env::set_current_dir("crates/secrets-nats-kv")?;
    tokio::process::Command::new("cargo")
        .arg("build")
        .output()
        .await?;
    std::env::set_current_dir("../../")?;
    tokio::process::Command::new("./target/debug/secrets-nats-kv")
        .args([
            "--encryption-xkey-seed",
            "SXAH5XWC6R6W52FLRWAQLK5C3VHXWBDYHKSROSJJBUS4T5HTW56FUCGECQ",
            "--transit-xkey-seed",
            "SXANK7TF7TNLYRQU2OOL6PZB6IGRX5PH75U55CIA4NWOBDPI3APXDGH7VY",
            "--subject-base",
            "wasmcloud.secrets",
            "--secrets-bucket",
            "TEST_SECRET_default",
            "--nats-address",
            nats_url.as_ref(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    // loop until request to wasmcloud.secrets.v0.nats-kv.server-xkey is successful
    // TODO: something more robust yadda yadda
    for _ in 0..10 {
        let resp = nats_client
            .request("wasmcloud.secrets.v0.nats-kv.server_xkey", "".into())
            .await
            .map_err(|e| {
                tracing::error!(?e);
                anyhow!("Request for server xkey failed")
            });
        if resp.map(|r| r.payload.len()).unwrap_or(0) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }

    // put secret in secret store
    let request_xkey = nkeys::XKey::new();
    let mut headers = HeaderMap::new();
    headers.insert(
        wasmcloud_secrets_types::WASMCLOUD_HOST_XKEY,
        request_xkey
            .public_key()
            .parse::<async_nats::HeaderValue>()
            .unwrap(),
    );
    let secret = wasmcloud_secrets_types::Secret {
        name: "ponger".to_string(),
        string_secret: Some("sup3rs3cr3t-v4lu3".to_string()),
        ..Default::default()
    };
    // TODO(#2344): we only really need this xkey to put the secret here but whatevs
    let transit_xkey =
        nkeys::XKey::from_seed("SXANK7TF7TNLYRQU2OOL6PZB6IGRX5PH75U55CIA4NWOBDPI3APXDGH7VY")
            .expect("valid");
    let transit_xkey_pub = nkeys::XKey::from_public_key(&transit_xkey.public_key()).expect("valid");
    let value = serde_json::to_string(&secret)?;
    let v = request_xkey
        .seal(value.as_bytes(), &transit_xkey_pub)
        .unwrap();
    let resp = nats_client
        .request_with_headers("wasmcloud.secrets.v0.nats-kv.put_secret", headers, v.into())
        .await?;

    eprintln!("resp: {:?}", String::from_utf8_lossy(&resp.payload));
    let put_resp: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(put_resp["revision"], 1);

    // Add mapping to allow the component to access the secrets
    // TODO: need to allow specifying host key?
    use tokio::io::AsyncReadExt;
    let mut component_bytes = vec![];
    let mut component = tokio::fs::File::open(RUST_PONGER_CONFIG_COMPONENT_PREVIEW2_SIGNED).await?;
    let _ = component.read_to_end(&mut component_bytes).await?;
    let component_token =
        wascap::wasm::extract_claims(&component_bytes)?.expect("claims to be valid");
    let component_key = nkeys::KeyPair::from_public_key(&component_token.claims.subject)?;

    let mut v: std::collections::HashSet<String> = std::collections::HashSet::new();
    v.insert("ponger".to_string());

    let payload = serde_json::to_string(&v).unwrap();
    let response = nats_client
        .request(
            format!(
                "wasmcloud.secrets.v0.nats-kv.add_mapping.{}",
                component_key.public_key()
            ),
            payload.into(),
        )
        .await?;
    println!("{:?}", response);
    assert_eq!(response.payload.to_vec(), b"ok");

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
        &host.host_key(),
        format!("file://{RUST_PINGER_CONFIG_COMPONENT_PREVIEW2_SIGNED}"),
        PINGER_COMPONENT_ID,
        None,
        5,
        vec!["pinger".to_string(), "pinger_override".to_string()],
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
    assert_config_put(
        &ctl_client,
        // NOTE: this follows the convention from https://github.com/wasmCloud/wadm/pull/307/files
        // Instead I'll create a helper function to make this better
        "secret_ponger",
        [
            ("key".to_string(), "ponger".to_string()),
            ("backend".to_string(), "nats-kv".to_string()),
            // ("version".to_string(), "v1".to_string()),
        ],
    )
    .await?;
    // Scale ponger
    assert_scale_component(
        &ctl_client,
        &host.host_key(),
        format!("file://{RUST_PONGER_CONFIG_COMPONENT_PREVIEW2_SIGNED}"),
        PONGER_COMPONENT_ID,
        None,
        5,
        vec!["ponger".to_string(), "secret_ponger".to_string()],
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
