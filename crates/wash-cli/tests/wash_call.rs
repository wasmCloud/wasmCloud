use anyhow::{Context, Result};
use serial_test::serial;

use wash_lib::cli::output::StartCommandOutput;

mod common;
use common::{TestWashInstance, HTTP_JSONIFY_OCI_REF};

use crate::common::wait_for_no_hosts;

/// Ensure that wash call works
#[tokio::test]
#[serial]
#[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
async fn integration_call_serial() -> Result<()> {
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;

    let instance = TestWashInstance::create().await?;

    // Pre-emptively pull the OCI ref for the component to ensure we don't run into the
    // default testing timeout when attempting to start the component
    let _ = instance
        .pull(HTTP_JSONIFY_OCI_REF)
        .await
        .context("failed to pull component")?;

    // Start an echo component
    let output: StartCommandOutput = instance
        .start_component(HTTP_JSONIFY_OCI_REF, "http-jsonify")
        .await
        .context("failed to start component")?;
    let component_id = output
        .component_id
        .context("component ID not present after starting component")?;

    // Call the component
    let output = instance
        .call_component(
            &component_id,
            "wasi:http/incoming-handler.handle",
            "body-data-goes-here",
        )
        .await
        .context("failed to call component")?;

    assert!(output.success, "call command succeeded");
    assert_eq!(output.response["status"], 200, "status code is 200");

    Ok(())
}
