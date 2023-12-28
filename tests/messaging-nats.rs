use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use futures::StreamExt;
use nkeys::KeyPair;
use serde::Deserialize;
use serde_json::json;
use tokio::fs;
use tokio::time::Duration;
use tokio::try_join;
use url::Url;
use wascap::jwt;

use wascap::wasm::extract_claims;
use wasmcloud_control_interface::ClientBuilder;
use wasmcloud_host::wasmbus::{Host, HostConfig};

pub mod common;
use common::{copy_par, free_port};

use crate::common::nats::start_nats;
use crate::common::{
    assert_advertise_link, assert_start_actor, assert_start_provider, stop_server,
};

const LATTICE_PREFIX: &str = "test-messaging-nats";

/// Test all functionality for the messaging-nats provider
///
/// - publish
/// - request
/// - handle-message (actor-side)
///
#[tokio::test(flavor = "multi_thread")]
async fn messaging_nats_suite() -> Result<()> {
    let ((nats_server, stop_nats_tx, nats_url, nats_client),) =
        try_join!(start_nats()).context("failed to start backing services")?;

    let httpserver_port = free_port().await?;
    let httpserver_base_url = format!("http://[{}]:{httpserver_port}", Ipv6Addr::LOCALHOST);

    // Get provider key/url for pre-built httpserver provider
    let httpserver_provider_key = KeyPair::from_seed(test_providers::RUST_HTTPSERVER_SUBJECT)
        .context("failed to parse `rust-httpserver` provider key")?;
    let (httpserver_provider_url, _httpserver_provider_tmp_path) =
        copy_par(test_providers::RUST_HTTPSERVER)
            .await
            .context("failed to build copied PAR")?;

    // Get provider key/url for pre-built messaging-nats provider (subject of this test)
    let messaging_nats_provider_key = KeyPair::from_seed(test_providers::RUST_NATS_SUBJECT)
        .context("failed to parse `rust-nats` provider key")?;
    let messaging_nats_provider_url = Url::from_file_path(test_providers::RUST_NATS)
        .map_err(|()| anyhow!("failed to construct provider ref"))?;

    // Get actor key/url for pre-built messaging-sender-http-smithy actor
    let messaging_sender_http_smithy_actor_url =
        Url::from_file_path(test_actors::RUST_MESSAGING_SENDER_HTTP_SMITHY_SIGNED)
            .map_err(|()| anyhow!("failed to construct messaging-sender-http-smithy actor ref"))?;

    // Get actor key/url for pre-built messaging-receiver-smithy actor
    let messaging_receiver_smithy_actor_url =
        Url::from_file_path(test_actors::RUST_MESSAGING_RECEIVER_SMITHY_SIGNED)
            .map_err(|()| anyhow!("failed to construct messaging-receiver-smithy actor ref"))?;

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

    // Subject to use for testing messages
    let subject = "test-subject";

    // Retrieve claims from sender actor
    let jwt::Token {
        claims: messaging_sender_http_smithy_claims,
        ..
    } = extract_claims(fs::read(test_actors::RUST_MESSAGING_SENDER_HTTP_SMITHY_SIGNED).await?)
        .context("failed to extract messaging sender http smithy actor claims")?
        .context("component actor claims missing")?;

    // Link the sender actor to both providers
    //
    // this must be done *before* the provider is started to avoid a race condition
    // to ensure the link is advertised before the actor would normally subscribe
    assert_advertise_link(
        &ctl_client,
        &messaging_sender_http_smithy_claims,
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
        &messaging_sender_http_smithy_claims,
        &messaging_nats_provider_key,
        "wasmcloud:messaging",
        "default",
        HashMap::from([(
            "config_json".into(),
            serde_json::to_string(&json!({
                "subscriptions": [], // sender actor only sends
                "cluster_uris": [ nats_url ],
            }))?,
        )]),
    )
    .await?;

    // Start the messaging-sender-http-smithy actor
    assert_start_actor(
        &ctl_client,
        &nats_client,
        LATTICE_PREFIX,
        &host_key,
        messaging_sender_http_smithy_actor_url,
        1,
    )
    .await?;

    // Retrieve claims from receiver actor
    let jwt::Token {
        claims: messaging_receiver_smithy_claims,
        ..
    } = extract_claims(fs::read(test_actors::RUST_MESSAGING_RECEIVER_SMITHY_SIGNED).await?)
        .context("failed to extract messaging receiver http smithy actor claims")?
        .context("component actor claims missing")?;

    assert_advertise_link(
        &ctl_client,
        &messaging_receiver_smithy_claims,
        &messaging_nats_provider_key,
        "wasmcloud:messaging",
        "default",
        HashMap::from([(
            "config_json".into(),
            serde_json::to_string(&json!({
                "subscriptions": [ subject ],
                "cluster_uris": [ nats_url ],
            }))?,
        )]),
    )
    .await?;

    // Start the messaging-receiver-smithy actor
    assert_start_actor(
        &ctl_client,
        &nats_client,
        LATTICE_PREFIX,
        &host_key,
        messaging_receiver_smithy_actor_url,
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

    // Start the messaging-nats provider
    assert_start_provider(
        &ctl_client,
        &nats_client,
        LATTICE_PREFIX,
        &host_key,
        &messaging_nats_provider_key,
        "default",
        messaging_nats_provider_url,
        None,
    )
    .await?;

    let http_client = reqwest::Client::default();

    // Set up a listener
    let listen_client = Arc::new(nats_client);
    let listener = tokio::task::spawn(async move {
        listen_client
            .subscribe(subject)
            .await
            .context("failed to create subscription")?
            .next()
            .await
            .ok_or_else(|| anyhow!("failed retrieve message"))
    });

    // Perform POST request to trigger a publish
    let resp_json: ResponseEnvelope<Option<()>> = http_client
        .post(format!("{httpserver_base_url}/publish"))
        .body(serde_json::to_string(&json!({
            "msg": {
                "subject": subject,
                "body": "hello world",
            }
        }))?)
        .send()
        .await
        .context("failed to perform POST /publish")?
        .json()
        .await
        .context("failed to read /publish response body as json")?;
    assert_eq!(resp_json.status, "success", "publish succeeded");

    let Ok(msg) = listener.await.context("wait failed for listener")? else {
        bail!("failed to listen to message");
    };
    assert!(
        msg.payload.iter().eq(b"hello world".iter()),
        "payload matches"
    );

    // Perform POST request to trigger a request, which should trigger the following:
    //
    // 1. listening httpserver provider receives the message
    // 2. messaging-sender-http-smithy is called to process the incoming http request
    // 3. messaging-sender-http-smithy sends a messaging request out using the wasmcloud:messaging contract
    // 4. messaging-receiver-smithy receives the message, and echoes back the contents
    //
    let resp_json: ResponseEnvelope<Message> = http_client
        .post(format!("{httpserver_base_url}/request"))
        .body(serde_json::to_string(&json!({
            "msg": {
                "subject": subject,
                "body": "hello world",
                "timeoutMs": 500,
            }
        }))?)
        .send()
        .await
        .context("failed to perform POST /request")?
        .json()
        .await
        .context("failed to read /request response body as json")?;
    assert_eq!(resp_json.status, "success", "request succeeded");
    assert_eq!(
        resp_json.data.body, b"hello world",
        "request/resp payload matches what was sent"
    );

    // Shutdown the host and backing services
    shutdown_host.await?;
    try_join!(stop_server(nats_server, stop_nats_tx)).context("failed to stop servers")?;

    Ok(())
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct ResponseEnvelope<T> {
    pub status: String,
    pub data: T,
}

/// A copy of the type defined in WIT (normally bindgen-generated)
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Message {
    subject: String,
    reply_to: Option<String>,
    #[serde(with = "serde_bytes")]
    body: Vec<u8>,
}

/// A copy of the type defined in WIT (normally bindgen-generated)
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct RequestMessage {
    subject: String,
    #[serde(with = "serde_bytes")]
    body: Vec<u8>,
    timeout_ms: u32,
}
