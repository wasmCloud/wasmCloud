use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use nkeys::KeyPair;
use tokio::fs;
use tokio::time::Duration;
use tokio::try_join;
use url::Url;
use wascap::jwt;

use wascap::wasm::extract_claims;
use wasmcloud_control_interface::ClientBuilder;
use wasmcloud_host::wasmbus::{Host, HostConfig};

pub mod common;
use common::free_port;

use crate::common::nats::start_nats;
use crate::common::vault::start_vault;
use crate::common::{
    assert_advertise_link, assert_start_actor, assert_start_provider, stop_server,
};

const LATTICE_PREFIX: &str = "test-kv-vault";

/// Test all functionality for the kv-vault provider
#[tokio::test(flavor = "multi_thread")]
async fn kv_vault_suite() -> Result<()> {
    // Start a Vault & NATS
    let vault_token = "test";
    let (
        (vault_server, stop_vault_tx, vault_url, vault_client),
        (nats_server, stop_nats_tx, nats_url, nats_client),
    ) = try_join!(start_vault(vault_token), start_nats())
        .context("failed to start backing services")?;

    let httpserver_port = free_port().await?;
    let httpserver_base_url = format!("http://[{}]:{httpserver_port}", Ipv6Addr::LOCALHOST);

    // Get provider key/url for pre-built httpserver provider
    let httpserver_provider_key = KeyPair::from_seed(test_providers::RUST_HTTPSERVER_SUBJECT)
        .context("failed to parse `rust-httpserver` provider key")?;
    let httpserver_provider_url = Url::from_file_path(test_providers::RUST_HTTPSERVER)
        .expect("failed to construct provider ref");

    // Get provider key/url for pre-built kv-vault provider (subject of this test)
    let kv_vault_provider_key = KeyPair::from_seed(test_providers::RUST_KV_VAULT_SUBJECT)
        .context("failed to parse `rust-kv-vault` provider key")?;
    let kv_vault_provider_url = Url::from_file_path(test_providers::RUST_KV_VAULT)
        .map_err(|()| anyhow!("failed to construct provider ref"))?;

    // Get actor key/url for pre-built kv-http-smithy actor
    let kv_http_smithy_actor_url = Url::from_file_path(test_actors::RUST_KV_HTTP_SMITHY_SIGNED)
        .map_err(|()| anyhow!("failed to construct actor ref"))?;

    // Build client for interacting with the lattice
    let ctl_client = ClientBuilder::new(nats_client.clone())
        .lattice_prefix(LATTICE_PREFIX.to_string())
        .build();

    // Start a wasmcloud host
    let cluster_key = Arc::new(KeyPair::new_cluster());
    let host_key = Arc::new(KeyPair::new_server());
    let (_host, shutdown_host) = Host::new(HostConfig {
        ctl_nats_url: nats_url.clone(),
        rpc_nats_url: nats_url.clone(),
        lattice_prefix: LATTICE_PREFIX.into(),
        cluster_key: Some(Arc::clone(&cluster_key)),
        cluster_issuers: Some(vec![cluster_key.public_key(), cluster_key.public_key()]),
        host_key: Some(Arc::clone(&host_key)),
        provider_shutdown_delay: Some(Duration::from_millis(300)),
        allow_file_load: true,
        ..Default::default()
    })
    .await
    .context("failed to initialize host")?;

    // Retrieve claims from actor
    let jwt::Token {
        claims: kv_http_smithy_claims,
        ..
    } = extract_claims(fs::read(test_actors::RUST_KV_HTTP_SMITHY_SIGNED).await?)
        .context("failed to extract kv http smithy actor claims")?
        .context("component actor claims missing")?;

    // Link the actor to both providers
    //
    // this must be done *before* the provider is started to avoid a race condition
    // to ensure the link is advertised before the actor would normally subscribe
    assert_advertise_link(
        &ctl_client,
        &kv_http_smithy_claims,
        &httpserver_provider_key,
        "wasmcloud:httpserver",
        "default",
        HashMap::from([(
            "config_json".into(),
            format!(
                r#"{{"address":"[{}]:{httpserver_port}"}}"#,
                Ipv6Addr::LOCALHOST,
            ),
        )]),
    )
    .await?;
    assert_advertise_link(
        &ctl_client,
        &kv_http_smithy_claims,
        &kv_vault_provider_key,
        "wasmcloud:keyvalue",
        "default",
        HashMap::from([
            ("ADDR".into(), vault_url.to_string()),
            ("TOKEN".into(), vault_token.to_string()),
        ]),
    )
    .await?;

    // Start the kv-http-smithy actor
    assert_start_actor(
        &ctl_client,
        &nats_client,
        LATTICE_PREFIX,
        &host_key,
        kv_http_smithy_actor_url,
        1,
    )
    .await?;

    // Start the HTTP provider
    assert_start_provider(
        &ctl_client,
        &nats_client,
        LATTICE_PREFIX,
        &host_key,
        &httpserver_provider_key,
        "default",
        httpserver_provider_url,
        None,
    )
    .await?;

    // Start the kv-vault provider
    assert_start_provider(
        &ctl_client,
        &nats_client,
        LATTICE_PREFIX,
        &host_key,
        &kv_vault_provider_key,
        "default",
        kv_vault_provider_url,
        None,
    )
    .await?;

    // Perform POST request to trigger a keyvalue get
    let http_client = reqwest::Client::default();
    let resp_json: ResponseEnvelope<GetResponseData> = http_client
        .post(format!("{httpserver_base_url}/get"))
        .body(r#"{"key": "test"}"#)
        .send()
        .await
        .context("failed to perform POST /get")?
        .json()
        .await
        .context("failed to read /get response body as json")?;
    assert_eq!(resp_json.status, "success", "initial get succeeded");
    assert!(!resp_json.data.exists);
    assert!(resp_json.data.value.is_empty());

    // Perform set request
    let http_client = reqwest::Client::default();
    let test_value = "example";
    let resp_json: ResponseEnvelope<SetResponseData> = http_client
        .post(format!("{httpserver_base_url}/set"))
        .body(format!(
            "{{\"key\": \"test\", \"value\": \"{test_value}\"}}"
        ))
        .send()
        .await
        .context("failed to perform POST /set")?
        .json()
        .await
        .context("failed to read /set response body as json")?;
    assert_eq!(resp_json.status, "success", "set succeeded");

    // Confirm the set worked with a get
    let resp_json: ResponseEnvelope<GetResponseData> = http_client
        .post(format!("{httpserver_base_url}/get"))
        .body(r#"{"key": "test"}"#)
        .send()
        .await
        .context("failed to perform POST /get")?
        .json()
        .await
        .context("failed to read /get response body as json")?;
    assert_eq!(resp_json.status, "success", "second get suceeded");
    assert!(resp_json.data.exists);
    assert_eq!(resp_json.data.value, test_value);

    // Perform contains
    let resp_json: ResponseEnvelope<ContainsResponseData> = http_client
        .post(format!("{httpserver_base_url}/contains"))
        .body(r#"{"key": "test"}"#)
        .send()
        .await
        .context("failed to perform POST /contains")?
        .json()
        .await
        .context("failed to read /contains response body as json")?;
    assert_eq!(resp_json.status, "success", "contains succeeded");
    assert!(resp_json.data);

    // Perform del
    let resp_json: ResponseEnvelope<DeleteResponseData> = http_client
        .post(format!("{httpserver_base_url}/del"))
        .body(r#"{"key": "test"}"#)
        .send()
        .await
        .context("failed to perform POST /del")?
        .json()
        .await
        .context("failed to read /del response body as json")?;
    assert_eq!(resp_json.status, "success", "del succeeded");
    assert!(resp_json.data);

    // Perform contains
    let resp_json: ResponseEnvelope<ContainsResponseData> = http_client
        .post(format!("{httpserver_base_url}/contains"))
        .body(r#"{"key": "test"}"#)
        .send()
        .await
        .context("failed to perform POST /contains (confirming delete)")?
        .json()
        .await
        .context("failed to read /contains response body as json")?;
    assert_eq!(
        resp_json.status, "success",
        "post-delete contains succeeded"
    );
    assert!(!resp_json.data);

    // Set a value in a set
    let http_client = reqwest::Client::default();
    let test_value = "example";
    let resp_json: ResponseEnvelope<SetResponseData> = http_client
        .post(format!("{httpserver_base_url}/set"))
        .body(format!(
            "{{\"key\": \"test_set/inner\", \"value\": \"{test_value}\"}}"
        ))
        .send()
        .await
        .context("failed to perform POST /set")?
        .json()
        .await
        .context("failed to read /set response body as json")?;
    assert_eq!(resp_json.status, "success", "set succeeded");

    // Perform set-query
    let resp_json: ResponseEnvelope<SetQueryResponseData> = http_client
        .post(format!("{httpserver_base_url}/set-query"))
        .body(r#"{"key": "test_set"}"#)
        .send()
        .await
        .context("failed to perform POST /set-query")?
        .json()
        .await
        .context("failed to read /set-query response body as json")?;
    assert_eq!(resp_json.status, "success", "list succeeded");
    assert_eq!(resp_json.data, Vec::from(["inner"]));

    // Test JSON serialization done by external clients
    let complex_value: HashMap<String, String> =
        HashMap::from([("one".into(), "1".into()), ("two".into(), "2".into())]);
    vaultrs::kv2::set(&vault_client, "secret", "ext_json", &complex_value)
        .await
        .context("failed to write secret with vault client directly")?;

    // Confirm the set worked with a get
    let resp_json: ResponseEnvelope<GetResponseData> = http_client
        .post(format!("{httpserver_base_url}/get"))
        .body(r#"{"key": "ext_json"}"#)
        .send()
        .await
        .context("failed to perform POST /get")?
        .json()
        .await
        .context("failed to read /get response body as json")?;
    assert_eq!(resp_json.status, "success", "second get suceeded");
    assert!(resp_json.data.exists);
    assert_eq!(
        serde_json::from_str::<HashMap<String, String>>(&resp_json.data.value)
            .context("failed to deserialize complex value from JSON")?,
        complex_value
    );

    // TODO: test reading value from ENV file (specified @ link time)
    // TODO: test renewal of token by introducing a new one and letting it expire..?
    // https://github.com/wasmCloud/capability-providers/commit/353e49b2e21ce8343bc90e1c9bc33986f63094ee

    // Shutdown the host and backing services
    shutdown_host.await?;
    try_join!(
        stop_server(vault_server, stop_vault_tx),
        stop_server(nats_server, stop_nats_tx),
    )
    .context("failed to stop servers")?;

    Ok(())
}

#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
struct ResponseEnvelope<T> {
    pub status: String,
    pub data: T,
}

#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
struct GetResponseData {
    exists: bool,
    value: String,
}

type DeleteResponseData = bool;
type SetResponseData = Option<()>;
type ContainsResponseData = bool;
type SetQueryResponseData = Vec<String>;
