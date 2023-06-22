use anyhow::{Context, Result};
use serial_test::serial;
use tokio::process::Command;
use wash_lib::cli::output::{
    GetClaimsCommandOutput, GetHostInventoryCommandOutput, GetHostsCommandOutput,
    LinkQueryCommandOutput,
};

mod common;
use common::TestWashInstance;

#[tokio::test]
#[serial]
async fn integration_get_hosts_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["get", "hosts", "--output", "json"])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute get hosts")?;

    assert!(output.status.success(), "executed get hosts query");

    let cmd_output: GetHostsCommandOutput = serde_json::from_slice(&output.stdout)?;
    assert!(cmd_output.success, "command returned success");
    assert_eq!(cmd_output.hosts.len(), 1, "hosts contains one host");
    assert_eq!(
        cmd_output.hosts[0].id, wash_instance.host_id,
        "single host ID matches has the wash ID"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_get_links_serial() -> Result<()> {
    let _wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["get", "links", "--output", "json"])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute get links")?;

    assert!(output.status.success(), "executed get links query");

    let cmd_output: LinkQueryCommandOutput = serde_json::from_slice(&output.stdout)?;
    assert!(cmd_output.success, "command returned success");
    assert_eq!(cmd_output.links.len(), 0, "links list is empty");

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_get_host_inventory_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "get",
            "inventory",
            &wash_instance.host_id,
            "--output",
            "json",
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute get inventory")?;

    assert!(output.status.success(), "executed get inventory");

    let cmd_output: GetHostInventoryCommandOutput = serde_json::from_slice(&output.stdout)?;
    assert!(cmd_output.success, "command returned success");

    assert!(
        cmd_output.inventory.actors.is_empty(),
        "host inventory contains no actors "
    );
    assert_eq!(
        cmd_output.inventory.host_id, wash_instance.host_id,
        "host ID matches request"
    );
    assert!(
        !cmd_output.inventory.labels.is_empty(),
        "at least one label on the host"
    );
    assert!(
        cmd_output.inventory.labels.contains_key("hostcore.os"),
        "hostcore.os label is present"
    );
    assert!(
        cmd_output.inventory.labels.contains_key("hostcore.arch"),
        "hostcore.arch label is present"
    );
    assert!(
        cmd_output
            .inventory
            .labels
            .contains_key("hostcore.osfamily"),
        "hostcore.osfmaily label is present"
    );
    assert!(
        cmd_output.inventory.providers.is_empty(),
        "host has no providers"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_get_claims_serial() -> Result<()> {
    let _wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(["get", "claims", "--output", "json"])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute get claims")?;

    assert!(output.status.success(), "executed get claims query");

    let cmd_output: GetClaimsCommandOutput = serde_json::from_slice(&output.stdout)?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}
