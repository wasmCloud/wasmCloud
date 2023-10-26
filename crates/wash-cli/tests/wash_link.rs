mod common;

use common::TestWashInstance;

use anyhow::{Context, Result};
use serial_test::serial;
use tokio::process::Command;
use wash_lib::cli::output::LinkQueryCommandOutput;

#[tokio::test]
#[serial]
async fn integration_link_serial() -> Result<()> {
    let wash = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "link",
            "query",
            "--output",
            "json",
            "--ctl-port",
            &wash.nats_port.to_string(),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute link query")?;

    assert!(output.status.success(), "executed link query");

    let cmd_output: LinkQueryCommandOutput = serde_json::from_slice(&output.stdout)?;
    assert!(cmd_output.success, "command returned success");
    assert_eq!(
        cmd_output.links.len(),
        0,
        "links list is empty without any links"
    );

    Ok(())
}
