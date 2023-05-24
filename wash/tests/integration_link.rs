use anyhow::{Context, Result};
use serde::Deserialize;
use serial_test::serial;
use std::collections::HashMap;
use wasmbus_rpc::core::ActorLinks;

mod common;
use common::{wash, TestWashInstance};

#[derive(Debug, Deserialize)]
struct LinkQueryOutput {
    links: Vec<HashMap<String, ActorLinks>>,
    success: bool,
}

#[tokio::test]
#[serial]
async fn integration_link() -> Result<()> {
    let _wash = TestWashInstance::create().await?;

    let output = wash()
        .args(["link", "query", "--output", "json"])
        .output()
        .context("failed to execute link query")?;

    assert!(output.status.success(), "executed link query");

    let cmd_output: LinkQueryOutput = serde_json::from_slice(&output.stdout)?;
    assert!(cmd_output.success, "command returned success");
    assert_eq!(
        cmd_output.links.len(),
        0,
        "links list is empty without any links"
    );

    Ok(())
}
