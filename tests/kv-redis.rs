use anyhow::{anyhow, Context, Result};
use nkeys::KeyPair;
use tokio::try_join;
use url::Url;

use wasmcloud_control_interface::ClientBuilder;
use wasmcloud_test_util::actor::extract_actor_claims;
use wasmcloud_test_util::host::WasmCloudTestHost;
use wasmcloud_test_util::lattice::link::assert_advertise_link;
use wasmcloud_test_util::provider::assert_start_provider;

pub mod common;

use crate::common::nats::start_nats;
use crate::common::redis::start_redis;

const LATTICE: &str = "test-kv-redis";

/// Test all functionality for the kv-redis provider
#[tokio::test(flavor = "multi_thread")]
async fn kv_redis_suite() -> Result<()> {
    // Start a Redis & NATS
    let _redis_token = "test";
    let ((redis_server, _redis_url), (nats_server, nats_url, nats_client)) =
        try_join!(start_redis(), start_nats()).context("failed to start backing services")?;

    // Get provider key/url for pre-built kv-redis provider (subject of this test)
    let kv_redis_provider_key = KeyPair::from_seed(test_providers::RUST_KVREDIS_SUBJECT)
        .context("failed to parse `rust-kv-redis` provider key")?;
    let kv_redis_provider_url = Url::from_file_path(test_providers::RUST_KVREDIS)
        .map_err(|()| anyhow!("failed to construct provider ref"))?;

    // // Get actor key/url for pre-built kv-http-smithy actor
    // let kv_http_smithy_actor_url = Url::from_file_path(test_actors::RUST_KV_HTTP_SMITHY_SIGNED)
    //     .map_err(|()| anyhow!("failed to construct actor ref"))?;

    // Build client for interacting with the lattice
    let ctl_client = ClientBuilder::new(nats_client.clone())
        .lattice(LATTICE.to_string())
        .build();

    // Build the host
    let host = WasmCloudTestHost::start(&nats_url, LATTICE, None, None)
        .await
        .context("failed to start test host")?;
    let kv_http_smithy_claims =
        extract_actor_claims(test_actors::RUST_KV_HTTP_SMITHY_SIGNED).await?;

    // Link the actor to both providers
    //
    // this must be done *before* the provider is started to avoid a race condition
    // to ensure the link is advertised before the actor would normally subscribe
    assert_advertise_link(
        &ctl_client,
        &kv_http_smithy_claims.subject,
        &kv_redis_provider_key.public_key(),
        "default",
        "wasi",
        "keyvalue",
        vec!["atomic".to_string(), "eventual".to_string()],
        vec![],
        vec![],
        // TODO: put configuration and reference here.
        // HashMap::from([
        //     ("ADDR".into(), redis_url.to_string()),
        //     ("TOKEN".into(), redis_token.to_string()),
        // ]),
    )
    .await?;

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

    // todo(vados-cosmonic): fix? starting actors seems to be broken on feat/wrpc?
    // // Start the kv-http-smithy actor
    // assert_start_actor(
    //     &ctl_client,
    //     host.host_key(),
    //     kv_http_smithy_actor_url.clone(),
    //     1,
    // )
    // .await?;

    // todo(vados-cosmonic): fix? starting actors seems to be broken on feat/wrpc?
    // // Scale the kv-http-smithy actor
    // assert_scale_actor(
    //     &ctl_client,
    //     host.host_key(),
    //     kv_http_smithy_actor_url,
    //     None,
    //     3,
    // )
    // .await?;

    // Stop host and backing services
    host.stop().await?;
    try_join!(redis_server.stop(), nats_server.stop()).context("failed to stop servers")?;

    Ok(())
}
