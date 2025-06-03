use std::sync::Arc;

use anyhow::{ensure, Context, Result};
use serial_test::serial;

use tokio::task::JoinSet;
use wash::lib::{
    cli::output::{CallCommandOutput, StartCommandOutput},
    context::WashContext,
};

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
        let response = output.response.context("missing response")?;
        if response
            .as_object()
            .is_some_and(|v| v.contains_key("status"))
        {
            assert_eq!(response["status"], 200, "status code is 200");
        };
    }

    Ok(())
}

/// Ensure that `wash call` works with an established context
#[tokio::test]
#[serial]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_call_with_context_serial() -> Result<()> {
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;

    let instance = Arc::new(TestWashInstance::create().await?);

    // Create a temporary wash context
    let ctx_name = "test";
    let contexts_dir = instance.test_dir().join("wash-contexts");
    instance.create_context(&contexts_dir, ctx_name).await?;
    let ctx_path = contexts_dir.join(format!("{ctx_name}.json"));

    // Read, modify, write a working context
    let mut ctx = serde_json::from_slice::<WashContext>(
        &tokio::fs::read(&ctx_path)
            .await
            .with_context(|| format!("failed to read context file @ [{}]", ctx_path.display()))?,
    )
    .context("failed to read context")?;
    ctx.ctl_port = instance.nats_port();
    ctx.rpc_port = instance.nats_port();
    tokio::fs::write(
        &ctx_path,
        serde_json::to_vec(&ctx).context("failed to serialize wash context")?,
    )
    .await
    .context("failed to write out modified context")?;

    // Pull and start the ferris component
    let _ = instance
        .pull(FERRIS_SAYS_OCI_REF)
        .await
        .context("failed to pull component")?;
    // Start the HTTP jsonify component which will use
    let output: StartCommandOutput = instance
        .start_component(FERRIS_SAYS_OCI_REF, "ferris-says")
        .await
        .context("failed to start component")?;
    let component_id = output
        .component_id
        .context("component ID not present after starting component")?;

    // Set up a wash command that with the randomized ctl port *missing*,
    // which would normally fail *except* we will set the context
    let mut cmd = instance.wash_cmd();
    cmd.env_remove("WASMCLOUD_CTL_PORT");

    // Perform the wash call (without context, this  should fail because it can't find the ctl port)
    let output = cmd
        .args([
            "call",
            &component_id,
            "wasmcloud:example-ferris-says/invoke.say",
            "--context-dir",
            &format!("{}", contexts_dir.display()),
            "--context",
            ctx_name,
            "--output",
            "json",
            "--http-body",
            "",
        ])
        .output()
        .await
        .context("failed to call ferris says w/ context")?;
    ensure!(output.status.success(), "wash call invocation failed");
    let output = serde_json::from_slice::<CallCommandOutput>(&output.stdout)
        .context("failed to parse output of `wash call` output")?;
    assert!(output.success, "wash call output parsed to success");

    instance.delete_context(contexts_dir, ctx_name).await?;

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
