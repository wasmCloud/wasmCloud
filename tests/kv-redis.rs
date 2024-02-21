use anyhow::{anyhow, Context, Result};
use nkeys::KeyPair;
use serde::{Deserialize, Serialize};
use tokio::try_join;
use url::Url;
use uuid::Uuid;

use wasmcloud_control_interface::ClientBuilder;
use wasmcloud_test_util::host::WasmCloudTestHost;
use wasmcloud_test_util::lattice::link::assert_advertise_link;
use wasmcloud_test_util::provider::assert_start_provider;
use wrpc_transport::Client;
use wrpc_transport_derive::EncodeSync;

pub mod common;

use crate::common::nats::start_nats;
use crate::common::redis::start_redis;

const LATTICE: &str = "test-kv-redis";

/// Test all functionality for the kv-redis provider
#[tokio::test(flavor = "multi_thread")]
async fn kv_redis_suite() -> Result<()> {
    // Start a Redis & NATS
    let _redis_token = "test";
    let ((redis_server, redis_url), (nats_server, nats_url, nats_client)) =
        try_join!(start_redis(), start_nats()).context("failed to start backing services")?;

    // Get provider key/url for pre-built kv-redis provider (subject of this test)
    let kv_redis_provider_key = KeyPair::from_seed(test_providers::RUST_KVREDIS_SUBJECT)
        .context("failed to parse `rust-kv-redis` provider key")?;
    let kv_redis_provider_url = Url::from_file_path(test_providers::RUST_KVREDIS)
        .map_err(|()| anyhow!("failed to construct provider ref"))?;

    // Build client for interacting with the lattice
    let ctl_client = ClientBuilder::new(nats_client.clone())
        .lattice(LATTICE.to_string())
        .build();
    // Build the client for interacting via wRPC
    let wrpc_client = wrpc_transport_nats::Client::new(
        nats_client.clone(),
        format!("{LATTICE}.{}", &kv_redis_provider_key.public_key()),
    );

    // Build the host
    let host = WasmCloudTestHost::start(&nats_url, LATTICE, None, None)
        .await
        .context("failed to start test host")?;

    // Start the kv-redis provider
    assert_start_provider(wasmcloud_test_util::provider::StartProviderArgs {
        client: &ctl_client,
        lattice: LATTICE,
        host_key: &host.host_key(),
        provider_key: &kv_redis_provider_key,
        provider_id: &kv_redis_provider_key.public_key(),
        url: &kv_redis_provider_url,
        configuration: None,
    })
    .await?;

    // Generate a random value we'll use to set
    let value = Uuid::new_v4();

    // Link fake actor --wasmcloud:keyvalue/key-value--> provider
    // todo(vados-cosmonic): use the wrapped wasmcloud_nats::Client here, so we can include headers
    assert_advertise_link(
        &ctl_client,
        "<unknown>",
        &kv_redis_provider_key.public_key(),
        "default",
        "wasmcloud",
        "keyvalue",
        vec!["key-value".to_string()],
        vec![],
        // todo(vados-cosmonic): this is a hack, remove and replace with named config!
        vec![format!("URL={}", redis_url.to_string())],
    )
    .await
    .context("advertise link (to fake actor) failed")?;

    // Trigger the kv-redis provider with wRPC directly
    let (_results, tx) = wrpc_client
        .invoke_static::<()>(
            "wasmcloud:keyvalue/key-value",
            "set",
            SetRequest {
                key: "test".into(),
                value: value.into(),
                expires: 0,
            },
        )
        .await
        .context("wasmcloud:keyvalue/key-value.set invocation failed")?;
    // Transmit parameters by awaiting transmit
    tx.await.context("failed to transmit parameters")?;

    // Use get to retrieve the value from redis
    let (results, tx) = wrpc_client
        .invoke_static::<String>("wasmcloud:keyvalue/key-value", "get", "test".to_string())
        .await
        .context("wasmcloud:keyvalue/key-value.set invocation failed")?;
    assert_eq!(
        results,
        value.to_string(),
        "value returned by get matched value saved by set"
    );
    // Transmit parameters by awaiting transmit
    tx.await.context("failed to transmit parameters")?;

    // Stop host and backing services
    host.stop().await?;
    try_join!(redis_server.stop(), nats_server.stop()).context("failed to stop servers")?;

    Ok(())
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize, EncodeSync)]
pub struct SetRequest {
    /// the key name to change (or create)
    #[serde(default)]
    pub key: String,
    /// the new value
    #[serde(default)]
    pub value: String,
    /// expiration time in seconds 0 for no expiration
    #[serde(default)]
    pub expires: u32,
}
