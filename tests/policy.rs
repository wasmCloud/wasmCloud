#![cfg(feature = "wasmcloud")]

use core::time::Duration;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use anyhow::Context as _;
use futures::StreamExt as _;
use serde_json::json;

use wasmcloud_host::wasmbus::host_config::PolicyService;
use wasmcloud_test_util::provider::{assert_start_provider, StartProviderArgs};
use wasmcloud_test_util::{component::assert_scale_component, host::WasmCloudTestHost};

pub mod common;
use common::nats::start_nats;
use common::providers;

use test_components::RUST_INTERFACES_REACTOR;

const LATTICE: &str = "default";

/// Ensure that a policy server configured with the host (that always denies)
/// successfully stops:
///
/// - starting providers
/// - starting components
#[tokio::test]
async fn policy_always_deny() -> anyhow::Result<()> {
    // Start NATS for communication
    let (nats_server, nats_url, nats_client) = start_nats(None, true)
        .await
        .map(|res| (res.0, res.1, res.2.unwrap()))
        .context("failed to start NATS")?;

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client)
        .lattice(LATTICE.to_string())
        .build();

    let rust_http_client = providers::rust_http_client().await;
    let rust_http_client_url = rust_http_client.url();
    let rust_http_client_id = rust_http_client.subject.public_key();

    // Build the host
    let host = WasmCloudTestHost::start_custom(
        &nats_url,
        LATTICE,
        None,
        None,
        // Since if a policy service is specified, the requests are deny-by-default
        // simply specifying one will cause all requests that require policy checks to fail
        Some(PolicyService {
            policy_topic: Some("test-policy".into()),
            policy_changes_topic: Some("test-policy-changes".into()),
            policy_timeout_ms: Some(Duration::from_millis(100)),
            policy_cache_ttl: None,
        }),
        None,
        None,
    )
    .await
    .context("failed to start test host")?;
    let host_key = host.host_key();

    assert!(
        assert_start_provider(StartProviderArgs {
            client: &ctl_client,
            host_id: &host_key.public_key(),
            provider_id: &rust_http_client_id,
            provider_ref: rust_http_client_url.as_str(),
            config: vec![],
        })
        .await
        .is_err(),
        "starting providers should fail"
    );
    assert!(
        assert_scale_component(
            &ctl_client,
            &host.host_key().public_key(),
            format!("file://{RUST_INTERFACES_REACTOR}"),
            "test-component",
            None,
            5,
            Vec::new(),
            Duration::from_secs(10),
        )
        .await
        .is_err(),
        "scaling actors should fail"
    );

    nats_server.stop().await.context("failed to stop NATS")?;
    Ok(())
}

#[tokio::test]
async fn policy_cache_respects_ttl() -> anyhow::Result<()> {
    let (nats_server, nats_url, nats_client) = start_nats(None, true)
        .await
        .map(|res| (res.0, res.1, res.2.unwrap()))
        .context("failed to start NATS")?;

    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client.clone())
        .lattice(LATTICE.to_string())
        .build();

    let policy_requests = Arc::new(AtomicUsize::new(0));
    let mut policy_sub = nats_client
        .subscribe("test-policy")
        .await
        .context("failed to subscribe to test policy topic")?;
    let policy_client = nats_client.clone();
    let policy_requests_for_task = Arc::clone(&policy_requests);
    let policy_server = tokio::spawn(async move {
        while let Some(msg) = policy_sub.next().await {
            let request = serde_json::from_slice::<serde_json::Value>(&msg.payload)
                .context("failed to deserialize policy request")?;
            let request_id = request
                .get("requestId")
                .and_then(serde_json::Value::as_str)
                .context("missing requestId in policy request")?;
            let permitted = policy_requests_for_task.fetch_add(1, Ordering::SeqCst) > 0;

            if let Some(reply) = msg.reply {
                policy_client
                    .publish(
                        reply,
                        serde_json::to_vec(&json!({
                            "requestId": request_id,
                            "permitted": permitted,
                        }))?
                        .into(),
                    )
                    .await
                    .context("failed to publish policy response")?;
            }
        }

        Ok::<(), anyhow::Error>(())
    });

    let rust_http_client = providers::rust_http_client().await;
    let rust_http_client_url = rust_http_client.url();
    let rust_http_client_id = rust_http_client.subject.public_key();

    let host = WasmCloudTestHost::start_custom(
        &nats_url,
        LATTICE,
        None,
        None,
        Some(PolicyService {
            policy_topic: Some("test-policy".into()),
            policy_changes_topic: None,
            policy_timeout_ms: Some(Duration::from_millis(100)),
            policy_cache_ttl: Some(Duration::from_millis(100)),
        }),
        None,
        None,
    )
    .await
    .context("failed to start test host")?;
    let host_key = host.host_key();

    assert!(
        assert_start_provider(StartProviderArgs {
            client: &ctl_client,
            host_id: &host_key.public_key(),
            provider_id: &rust_http_client_id,
            provider_ref: rust_http_client_url.as_str(),
            config: vec![],
        })
        .await
        .is_err(),
        "first start should be denied by the policy response"
    );

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(
        assert_start_provider(StartProviderArgs {
            client: &ctl_client,
            host_id: &host_key.public_key(),
            provider_id: &rust_http_client_id,
            provider_ref: rust_http_client_url.as_str(),
            config: vec![],
        })
        .await
        .is_ok(),
        "second start should re-query policy after the cache TTL expires"
    );
    assert_eq!(policy_requests.load(Ordering::SeqCst), 2);

    host.stop().await.context("failed to stop host")?;
    policy_server.abort();
    nats_server.stop().await.context("failed to stop NATS")?;
    Ok(())
}
