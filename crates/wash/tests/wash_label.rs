mod common;

use common::TestWashInstance;

use anyhow::{Context, Result};
use serial_test::serial;
use tokio::process::Command;
use wash::lib::cli::output::LabelHostCommandOutput;

#[tokio::test]
#[serial]
async fn integration_label_host_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "label",
            &wash_instance.host_id,
            "key1=value1",
            "--output",
            "json",
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute wash label")?;

    let cmd_output: LabelHostCommandOutput = serde_json::from_slice(&output.stdout)?;
    assert!(cmd_output.success, "command returned success");

    assert!(!cmd_output.deleted);
    assert_eq!(
        cmd_output.processed,
        vec![(String::from("key1"), String::from("value1"))],
    );
    Ok(())
}
