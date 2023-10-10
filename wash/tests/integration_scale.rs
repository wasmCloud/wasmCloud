use anyhow::{Context, Result};
use serial_test::serial;
use tokio::process::Command;

mod common;
use common::{TestWashInstance, ECHO_OCI_REF};
use wash_lib::cli::output::{GetHostInventoryCommandOutput, ScaleCommandOutput};

#[tokio::test]
#[serial]
async fn integration_scale_actor_serial() -> Result<()> {
    let wash_instance = TestWashInstance::create().await?;

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        // New syntax with --max and no `ctl` prefix
        .args([
            "scale",
            "actor",
            wash_instance.host_id.as_str(),
            ECHO_OCI_REF,
            "--max",
            "10",
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
        .context("failed to scale actor")?;

    assert!(output.status.success(), "executed scale");

    let cmd_output: ScaleCommandOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse output")?;
    assert!(cmd_output.success, "command returned accepted");

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

        let cmd_output: GetHostInventoryCommandOutput =
            serde_json::from_slice(&output.stdout).context("failed to parse output")?;

        if cmd_output.inventory.actors.is_empty() && retries > 4 {
            panic!("Should have started the actor")
        } else if retries <= 4 {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            continue;
        } else {
            assert_eq!(cmd_output.inventory.actors.len(), 1);
            let max = cmd_output.inventory.actors[0]
                .instances
                .iter()
                .map(|i| {
                    if i.max_concurrent == 0 {
                        1
                    } else {
                        i.max_concurrent
                    }
                })
                .sum::<u16>();
            assert_eq!(max, 10);
            break;
        }
    }

    let output = Command::new(env!("CARGO_BIN_EXE_wash"))
        // Old syntax with --count and a `ctl` prefix
        .args([
            "ctl",
            "scale",
            "actor",
            wash_instance.host_id.as_str(),
            ECHO_OCI_REF,
            "--count",
            "5",
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
        .context("failed to scale actor")?;

    assert!(output.status.success(), "executed scale");

    let cmd_output: ScaleCommandOutput =
        serde_json::from_slice(&output.stdout).context("failed to parse output")?;
    assert!(cmd_output.success, "command returned accepted");

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

        let cmd_output: GetHostInventoryCommandOutput =
            serde_json::from_slice(&output.stdout).context("failed to parse output")?;

        if cmd_output.inventory.actors.is_empty() && retries > 4 {
            panic!("Should have started the actor")
        } else if retries <= 4 {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            continue;
        } else {
            assert_eq!(cmd_output.inventory.actors.len(), 1);
            // NOTE: This is a compensation for the fact that <0.79.0 hosts include individual instances,
            // but 0.79.0+ hosts include instances with max counts on them.
            let max = cmd_output.inventory.actors[0]
                .instances
                .iter()
                .map(|i| {
                    if i.max_concurrent == 0 {
                        1
                    } else {
                        i.max_concurrent
                    }
                })
                .sum::<u16>();
            assert_eq!(max, 5);
            break;
        }
    }

    Ok(())
}
