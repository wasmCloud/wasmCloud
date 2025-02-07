use std::sync::Arc;

use anyhow::{Context, Result};
use serial_test::serial;

use tokio::task::JoinSet;
use wash::lib::cli::output::{CallCommandOutput, StartCommandOutput};

mod common;
use common::{TestWashInstance, FERRIS_SAYS_OCI_REF, HTTP_JSONIFY_OCI_REF};

use crate::common::wait_for_no_hosts;

/// Ensure that `wash call` works for a few use cases
#[tokio::test]
#[serial]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_call_serial() -> Result<()> {
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;

    let instance = Arc::new(TestWashInstance::create().await?);

    let mut outputs = JoinSet::new();
    outputs.spawn(pull_and_call(
        instance.clone(),
        HTTP_JSONIFY_OCI_REF,
        "http-jsonify",
        "wasi:http/incoming-handler.handle",
        "body-data-goes-here",
    ));
    outputs.spawn(pull_and_call(
        instance.clone(),
        FERRIS_SAYS_OCI_REF,
        "ferris-says",
        "wasmcloud:example-ferris-says/invoke.say",
        "",
    ));

    while let Some(Ok(Ok(output))) = outputs.join_next().await {
        assert!(output.success, "call command succeeded");
        if output
            .response
            .as_object()
            .is_some_and(|v| v.contains_key("status"))
        {
            assert_eq!(output.response["status"], 200, "status code is 200");
        }
    }

    Ok(())
}

/// Utility function for pulling and calling a component
async fn pull_and_call(
    instance: Arc<TestWashInstance>,
    component_ref: &str,
    name: &str,
    operation: &str,
    input: &str,
) -> Result<CallCommandOutput> {
    // Pre-emptively pull the OCI ref for the component to ensure we don't run into the
    // default testing timeout when attempting to start the component
    let _ = instance
        .pull(component_ref)
        .await
        .context("failed to pull component")?;

    // Start the HTTP jsonify component which will use
    let output: StartCommandOutput = instance
        .start_component(component_ref, name)
        .await
        .context("failed to start component")?;
    let component_id = output
        .component_id
        .context("component ID not present after starting component")?;

    // Call the component
    instance
        .call_component(&component_id, operation, input)
        .await
        .context("failed to call component")
}
