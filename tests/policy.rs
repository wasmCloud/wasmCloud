use core::time::Duration;

use anyhow::Context as _;

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
    let (nats_server, nats_url, _, nats_client_0_33) =
        start_nats().await.context("failed to start NATS")?;

    // Build client for interacting with the lattice
    let ctl_client = wasmcloud_control_interface::ClientBuilder::new(nats_client_0_33)
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
        }),
    )
    .await
    .context("failed to start test host")?;
    let host_key = host.host_key();

    assert!(
        assert_start_provider(StartProviderArgs {
            client: &ctl_client,
            lattice: LATTICE,
            host_key: &host_key,
            provider_key: &rust_http_client.subject,
            provider_id: &rust_http_client_id,
            url: &rust_http_client_url,
            config: vec![],
        })
        .await
        .is_err(),
        "starting providers should fail"
    );

    assert!(
        assert_scale_component(
            &ctl_client,
            &host.host_key(),
            format!("file://{RUST_INTERFACES_REACTOR}"),
            "test-component",
            None,
            5,
            Vec::new(),
        )
        .await
        .is_err(),
        "scaling actors should fail"
    );

    nats_server.stop().await.context("failed to stop NATS")?;
    Ok(())
}
