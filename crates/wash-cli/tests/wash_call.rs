use anyhow::{Context, Result};
use serial_test::serial;

use wash_lib::cli::output::StartCommandOutput;

mod common;
use common::{TestWashInstance, HTTP_JSONIFY_OCI_REF};

/// Ensure that wash call works
#[tokio::test]
#[serial]
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
async fn integration_call() -> Result<()> {
    let instance = TestWashInstance::create().await?;

    // Start an echo component
    let StartCommandOutput { component_id, .. } = instance
        .start_component(HTTP_JSONIFY_OCI_REF, "http-jsonify")
        .await
        .context("failed to start component")?;
    let component_id = component_id.context("component ID not present after starting component")?;

    // Build request payload to send to the echo component
    let request = serde_json::json!({
        "method": "GET",
        "path": "/",
        "body": "",
        "queryString": "",
        "header": {},
    });

    // Call the component
    let cmd_output = instance
        .call_component(
            &component_id,
            "wasi:http/incoming-handler.handle",
            serde_json::to_string(&request).context("failed to convert wash call data")?,
        )
        .await
        .context("failed to call component")?;
    assert!(cmd_output.success, "call command succeeded");
    assert_eq!(cmd_output.response["status"], 200, "status code is 200");

    Ok(())
}
