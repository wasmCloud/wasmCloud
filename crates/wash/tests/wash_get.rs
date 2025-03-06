mod common;

use common::TestWashInstance;

use anyhow::{bail, Context, Result};
use serial_test::serial;
use tokio::process::Command;
use wash::lib::cli::output::{
    GetClaimsCommandOutput, GetHostInventoriesCommandOutput, GetHostsCommandOutput,
    LinkQueryCommandOutput,
};

#[tokio::test]
#[serial]
async fn integration_get_hosts_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "get",
            "hosts",
            "--output",
            "json",
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute get hosts")?;

    assert!(output.status.success(), "executed get hosts query");

    let cmd_output: GetHostsCommandOutput = serde_json::from_slice(&output.stdout)?;
    assert!(cmd_output.success, "command returned success");
    assert_eq!(cmd_output.hosts.len(), 1, "hosts contains one host");
    assert_eq!(
        cmd_output.hosts[0].id(),
        wash_instance.host_id,
        "single host ID matches has the wash ID"
    );

    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_get_links_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "get",
            "links",
            "--output",
            "json",
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
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
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute get inventory")?;

    if !output.status.success() {
        bail!(
            "failed to execute `wash get inventory`, stdout: {} \nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let cmd_output: GetHostInventoriesCommandOutput = serde_json::from_slice(&output.stdout)?;
    assert!(cmd_output.success, "command returned success");

    assert!(
        cmd_output.inventories.len() == 1,
        "one host inventory returned"
    );
    let inventory = &cmd_output.inventories[0];
    assert!(
        inventory.components().is_empty(),
        "host inventory contains no components "
    );
    assert_eq!(
        inventory.host_id(),
        wash_instance.host_id,
        "host ID matches request"
    );
    assert!(
        !inventory.labels().is_empty(),
        "at least one label on the host"
    );
    assert!(
        inventory.labels().contains_key("hostcore.os"),
        "hostcore.os label is present"
    );
    assert!(
        inventory.labels().contains_key("hostcore.arch"),
        "hostcore.arch label is present"
    );
    assert!(
        inventory.labels().contains_key("hostcore.osfamily"),
        "hostcore.osfmaily label is present"
    );
    assert!(inventory.providers().is_empty(), "host has no providers");

    Ok(())
}

#[tokio::test]
#[serial]
// TODO: reenable after #1649 merges and v1.0.0-alpha.2 is released
// This issue was fixed in 08bb43a8ae90dc83db653ed78b039479ffe1dd2e
#[ignore]
async fn integration_get_claims_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "get",
            "claims",
            "--output",
            "json",
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute get claims")?;

    assert!(output.status.success(), "executed get claims query");

    let cmd_output: GetClaimsCommandOutput = serde_json::from_slice(&output.stdout)?;
    assert!(cmd_output.success, "command returned success");

    Ok(())
}

/// Ensure that labels on host inventories are sorted
#[tokio::test]
#[serial]
async fn integration_get_host_inventory_labels_sorted_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create_with_extra_args(vec![
        "--label", "three=3", "--label", "two=2", "--label", "one=1",
    ])
    .await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "get",
            "inventory",
            &wash_instance.host_id,
            "--output",
            "json",
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to execute get inventory")?;

    if !output.status.success() {
        bail!(
            "failed to execute `wash get inventory`, stdout: {} \nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Since it's very likely that the output itself will contain lines that
    // have our JSON key, we search for each expected key individually and make sure they came out in an expected (lexicographical order)
    let one_idx = stdout
        .find(r#""one": "1""#)
        .context("missing JSON list entry for one")?;
    let two_idx = stdout
        .find(r#""three": "3""#)
        .context("missing JSON list entry for two")?;
    let three_idx = stdout
        .find(r#""two": "2""#)
        .context("missing JSON list entry for three")?;
    assert!(
        one_idx < two_idx && two_idx < three_idx,
        "one before two before three "
    );
    Ok(())
}
