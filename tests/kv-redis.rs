use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use nkeys::KeyPair;
use serde_json::json;
use tokio::try_join;
use tracing::debug;
use url::Url;
use uuid::Uuid;

use wasmcloud_compat::keyvalue::GetResponse;
use wasmcloud_control_interface::ClientBuilder;
use wasmcloud_test_util::actor::assert_scale_actor;
use wasmcloud_test_util::host::WasmCloudTestHost;
use wasmcloud_test_util::lattice::config::assert_config_put;
use wasmcloud_test_util::lattice::link::assert_advertise_link;
use wasmcloud_test_util::provider::assert_start_provider;

pub mod common;

use crate::common::nats::start_nats;
use crate::common::redis::start_redis;

const LATTICE: &str = "test-kv-redis";
const MESSAGING_INVOKER_COMPONENT_ID: &str = "messaging-invoker";

/// Test all functionality for the kv-redis provider
///
/// This test works by:
/// - starting a single-node NATS cluster
/// - starting a redis instance
/// - starting a messaging provider, kv-redis provider, and messaging-invoker actor
///
/// The test sends messages over NATS which are picked up by the messaging provider and forwarded to the invoker
/// the invokers job is then to invoke the appropriate function on the keyvalue redis provider.
///
/// As such, this test communications paths for:
/// - provider -> actor
/// - actor -> provider
#[tokio::test(flavor = "multi_thread")]
async fn kv_redis_suite() -> Result<()> {
    let _ = tracing_subscriber::fmt().try_init();

    // Start a Redis & NATS
    let ((redis_server, redis_url), (nats_server, nats_url, nats_client)) =
        try_join!(start_redis(), start_nats()).context("failed to start backing services")?;

    // Get provider key/url for pre-built kv-redis provider (subject of this test)
    let kv_redis_provider_key = KeyPair::from_seed(test_providers::RUST_KVREDIS_SUBJECT)
        .context("failed to parse `rust-kv-redis` provider key")?;
    let kv_redis_provider_url = Url::from_file_path(test_providers::RUST_KVREDIS)
        .map_err(|()| anyhow!("failed to construct provider ref"))?;

    // Get provider key/url for pre-built kv-redis provider (subject of this test)
    let messaging_nats_provider_key = KeyPair::from_seed(test_providers::RUST_NATS_SUBJECT)
        .context("failed to parse `rust-kv-redis` provider key")?;
    let messaging_nats_provider_url = Url::from_file_path(test_providers::RUST_NATS)
        .map_err(|()| anyhow!("failed to construct provider ref"))?;

    // Get actor key/url for pre-built messaging-invoker actor component
    let messaging_invoker_actor_url =
        Url::from_file_path(test_actors::RUST_MESSAGING_INVOKER_COMPONENT_PREVIEW2_SIGNED)
            .map_err(|()| anyhow!("failed to construct messaging invoker actor ref"))?;

    // Build client for interacting with the lattice
    let ctl_client = ClientBuilder::new(nats_client.clone())
        .lattice(LATTICE.to_string())
        .build();

    // Build the host
    let host = WasmCloudTestHost::start(&nats_url, LATTICE, None, None)
        .await
        .context("failed to start test host")?;

    // Generate a random test subject
    let test_subject = Uuid::new_v4().to_string();

    // Scale messaging invoker
    // NOTE: we *must* have ONLY one actor, as we will use operations that ask a specific
    // actor to recall values it has seen
    assert_scale_actor(
        &ctl_client,
        &host.host_key(),
        messaging_invoker_actor_url,
        "messaging-invoker",
        None,
        1,
        Vec::new(),
    )
    .await
    .context("should've scaled actor")?;

    // Start the messaging-nats provider
    assert_start_provider(wasmcloud_test_util::provider::StartProviderArgs {
        client: &ctl_client,
        lattice: LATTICE,
        host_key: &host.host_key(),
        provider_key: &messaging_nats_provider_key,
        provider_id: &messaging_nats_provider_key.public_key(),
        url: &messaging_nats_provider_url,
        config: vec![],
    })
    .await?;

    // Start the kv-redis provider
    assert_start_provider(wasmcloud_test_util::provider::StartProviderArgs {
        client: &ctl_client,
        lattice: LATTICE,
        host_key: &host.host_key(),
        provider_key: &kv_redis_provider_key,
        provider_id: &kv_redis_provider_key.public_key(),
        url: &kv_redis_provider_url,
        config: vec![],
    })
    .await?;

    assert_config_put(
        &ctl_client,
        "MESSAGING_NATS",
        HashMap::from_iter([
            ("CLUSTER_URI".to_string(), nats_url.to_string()),
            ("SUBSCRIPTIONS".to_string(), test_subject.clone()),
        ]),
    )
    .await?;

    // Link messaging-invoker ---[wasmcloud:messaging/message-subscriber]---> messaging provider
    assert_advertise_link(
        &ctl_client,
        MESSAGING_INVOKER_COMPONENT_ID,
        messaging_nats_provider_key.public_key(),
        "default",
        "wasmcloud",
        "messaging",
        vec!["messaging".to_string(), "message-subscriber".to_string()],
        vec![],
        // NOTE: this should be temporary, rather than using a named config,
        // we are stuffing credentials into the target_config
        vec![
            format!("subscriptions={test_subject}"),
            format!("cluster_uris={nats_url}"),
        ],
    )
    .await
    .context("should advertise link")?;

    // Link messaging-invoker ---[wasmcloud:keyvalue/key-value]---> messaging provider
    assert_advertise_link(
        &ctl_client,
        MESSAGING_INVOKER_COMPONENT_ID,
        kv_redis_provider_key.public_key(),
        "default",
        "wasmcloud",
        "keyvalue",
        vec!["key-value".to_string()],
        vec![],
        // NOTE: this should be temporary, rather than using a named config,
        // we are stuffing credentials into the target_config
        vec![format!("URL={}", redis_url.to_string())],
    )
    .await
    .context("should advertise link")?;

    // Wait a bit for links to be established, we need:
    // - messaging provider to connect and start listening
    // - redis provider to connect to redis
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    // Generate a random value to actually set the redis key to
    let key = Uuid::new_v4().to_string();
    let value = Uuid::new_v4().to_string();

    // Send NATS message on generated test topic that should be picked up by provider
    // and forwarded to messaging-invoker
    debug!(key, value, "triggering set");
    let set_resp = nats_client
        .request(
            test_subject.clone(),
            Bytes::from(
                serde_json::to_vec(&json!({
                "link_name": "default",
                "wit_ns": "wasmcloud",
                "wit_pkg": "keyvalue",
                "wit_iface": "key-value",
                "wit_fn": "set",
                "params_json_b64": [
                    STANDARD
                        .encode(
                            serde_json::to_vec(&json!({
                                "key": key,
                                "value": value,
                                "expires": 0u32,
                            }))
                                .context("failed to encode set-request")?
                        )
                ],
                            }))
                .context("failed to encode json")?,
            ),
        )
        .await
        .context("failed to publish invoke triggering message")?;

    // Ensure that the response from set (which is normally nothing) was sent back as
    // rust unit type, i.e empty bytes
    assert!(set_resp.payload.is_empty(), "returned set payload is empty");

    // Perform the GET of the message we just set
    debug!(key, "triggering get");
    let get_resp = nats_client
        .request(
            test_subject.clone(),
            Bytes::from(
                serde_json::to_vec(&json!({
                "link_name": "default",
                "wit_ns": "wasmcloud",
                "wit_pkg": "keyvalue",
                "wit_iface": "key-value",
                "wit_fn": "get",
                "params_json_b64": [
                    STANDARD
                        .encode(
                            serde_json::to_vec(&json!(key)).context("failed to encode get")?
                        )
                ],
                            }))
                .context("failed to encode json")?,
            ),
        )
        .await
        .context("failed to publish invoke triggering message")?;

    // Check the response from the get
    let get_resp = serde_json::from_slice::<GetResponse>(&get_resp.payload)
        .context("failed to parse payload from set")?;
    assert_eq!(get_resp.value, value);
    assert!(get_resp.exists);

    // Stop host and backing services
    host.stop().await?;
    try_join!(redis_server.stop(), nats_server.stop()).context("failed to stop servers")?;

    Ok(())
}
