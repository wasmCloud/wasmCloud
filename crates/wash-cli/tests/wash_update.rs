mod common;

use common::{TestWashInstance, ECHO_OCI_REF};

use anyhow::{Context, Result};
use serial_test::serial;
use tokio::process::Command;
use wash_lib::cli::output::{GetHostInventoriesCommandOutput, StartCommandOutput};

const OLD_ECHO_OCI_REF: &str = "wasmcloud.azurecr.io/echo:0.3.4";
const ECHO_ACTOR_ID: &str = "MBCFOPM6JW2APJLXJD3Z5O4CN7CPYJ2B4FTKLJUR5YR5MITIU7HD3WD5";

#[tokio::test]
#[serial]
#[cfg_attr(
    not(can_reach_wasmcloud_azurecr_io),
    ignore = "wasmcloud.azurecr.io is not reachable"
)]
async fn integration_update_actor_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "start",
            "actor",
            OLD_ECHO_OCI_REF,
            "--output",
            "json",
            "--timeout-ms",
            "40000",
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to start actor")?;

    assert!(output.status.success(), "executed start");

    let cmd_output: StartCommandOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse output")?;
    assert!(cmd_output.success, "command returned success");

    // Give the host a couple of seconds to download the actor bytes and start the actor
    for retries in 0..5 {
        // get host inventory
        let output = Command::new(env!("CARGO_BIN_EXE_wash"))
            .args([
                "get",
                "inventory",
                "--output",
                "json",
                "--timeout-ms",
                "2000",
                "--ctl-port",
                &wash_instance.nats_port.to_string(),
            ])
            .kill_on_drop(true)
            .output()
            .await
            .context("failed to get host inventory")?;
        assert!(output.status.success(), "checked host inventory");

        let cmd_output: GetHostInventoriesCommandOutput =
            serde_json::from_slice(&output.stdout).context("failed to parse output")?;

        let actors = cmd_output
            .inventories
            .into_iter()
            .next()
            .map(|i| i.actors)
            .unwrap_or_default();
        if actors.is_empty() && retries > 4 {
            panic!("Should have started the actor")
        } else if retries <= 4 {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            continue;
        } else {
            assert_eq!(actors.len(), 1);
            assert!(actors[0]
                .image_ref
                .as_ref()
                .is_some_and(|image_ref| image_ref == OLD_ECHO_OCI_REF));
            break;
        }
    }

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "update",
            "actor",
            wash_instance.host_id.as_str(),
            ECHO_ACTOR_ID,
            ECHO_OCI_REF,
            "--output",
            "json",
            "--timeout-ms",
            "40000",
            "--ctl-port",
            &wash_instance.nats_port.to_string(),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("failed to start actor")?;

    assert!(output.status.success(), "executed start");

    let cmd_output: StartCommandOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse output")?;
    assert!(cmd_output.success, "command returned success");

    // Give the host a couple of seconds to download the actor bytes and start the actor
    for retries in 0..5 {
        // get host inventory
        let output = Command::new(env!("CARGO_BIN_EXE_wash"))
            .args([
                "get",
                "inventory",
                "--output",
                "json",
                "--timeout-ms",
                "2000",
                "--ctl-port",
                &wash_instance.nats_port.to_string(),
            ])
            .kill_on_drop(true)
            .output()
            .await
            .context("failed to get host inventory")?;
        assert!(output.status.success(), "checked host inventory");

        let cmd_output: GetHostInventoriesCommandOutput =
            serde_json::from_slice(&output.stdout).context("failed to parse output")?;

        // SAFETY: This is a test but also since the actor already started we should never get
        // no actors returned here. Give the host a few retries here, the old actor should still
        // be running the whole time.
        let actors = cmd_output
            .inventories
            .into_iter()
            .next()
            .map(|i| i.actors)
            .unwrap_or_default();
        if !actors[0]
            .image_ref
            .as_ref()
            .is_some_and(|image_ref| image_ref == ECHO_OCI_REF)
            && retries > 4
        {
            panic!("Should have started the actor")
        } else if retries <= 4 {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            continue;
        } else {
            assert_eq!(actors.len(), 1);
            assert!(actors[0]
                .image_ref
                .as_ref()
                .is_some_and(|image_ref| image_ref == ECHO_OCI_REF));
            break;
        }
    }

    Ok(())
}
