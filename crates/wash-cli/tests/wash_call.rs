use anyhow::{Context, Result};
use serial_test::serial;
use wash_lib::cli::output::StartCommandOutput;

mod common;
use common::{TestWashInstance, ECHO_OCI_REF};

/// Ensure that wash call works
#[tokio::test]
#[serial]
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
async fn integration_call() -> Result<()> {
    let instance = TestWashInstance::create().await?;

    // Start an echo actor
    let StartCommandOutput { actor_id, .. } = instance
        .start_actor(ECHO_OCI_REF)
        .await
        .context("failed to start actor")?;
    let actor_id = actor_id.context("actor ID not present after starting actor")?;

    // Build request payload to send to the echo actor
    let request = serde_json::json!({
        "method": "GET",
        "path": "/",
        "body": "",
        "queryString": "",
        "header": {},
    });

    // Call the actor
    let cmd_output = instance
        .call_actor(
            &actor_id,
            "HttpServer.HandleRequest",
            serde_json::to_string(&request).context("failed to convert wash call data")?,
        )
        .await
        .context("failed to call actor")?;
    assert!(cmd_output.success, "call command succeeded");
    assert_eq!(cmd_output.response["statusCode"], 200, "status code is 200");

    Ok(())
}
