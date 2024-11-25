use anyhow::{Context as _, Result};

use wasmcloud_test_util::control_interface::ClientBuilder;
use wasmcloud_test_util::{assert_config_put, assert_scale_component, WasmCloudTestHost};

mod common;
use common::nats::start_nats;

/// Ensure the code from the quickstart example works
///
/// This test is ignored by default as it requires a NATS server running in the background.
#[tokio::test]
#[ignore]
async fn test_quickstart() -> Result<()> {
    let (nats_server, nats_url, nats_client) = start_nats().await?;

    let lattice = "default";
    let host = WasmCloudTestHost::start(nats_url, lattice)
        .await
        .context("failed to start host")?;

    let ctl_client = ClientBuilder::new(nats_client)
        .lattice(host.lattice_name().to_string())
        .build();

    assert_config_put(
        &ctl_client,
        "test-config",
        [("EXAMPLE_KEY".to_string(), "EXAMPLE_VALUE".to_string())],
    )
    .await
    .context("failed to put config")?;

    assert_scale_component(
        &ctl_client,
        &host.host_key().public_key(),
        "ghcr.io/wasmcloud/components/http-jsonify-rust:0.1.1",
        "example-component",
        None,
        1,
        Vec::new(),
        tokio::time::Duration::from_secs(10),
    )
    .await
    .context("failed to start component")?;

    nats_server
        .stop()
        .await
        .context("failed to stop NATS server")?;

    Ok(())
}
