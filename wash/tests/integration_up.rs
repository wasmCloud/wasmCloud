use serial_test::serial;
use std::fs::{read_to_string, remove_dir_all};

use anyhow::{anyhow, Context, Result};
use common::test_dir_with_subfolder;
use regex::Regex;
use tokio::{process::Command, time::Duration};

mod common;

use common::{start_nats, wait_for_nats_to_start, wait_for_no_hosts, wait_for_single_host};

const RGX_ACTOR_START_MSG: &str = r"Actor \[(?P<actor_id>[^]]+)\] \(ref: \[(?P<actor_ref>[^]]+)\]\) started on host \[(?P<host_id>[^]]+)\]";

#[tokio::test]
#[serial]
async fn integration_up_can_start_wasmcloud_and_actor_serial() -> Result<()> {
    let dir = test_dir_with_subfolder("can_start_wasmcloud");
    let path = dir.join("washup.log");
    let stdout = std::fs::File::create(&path).expect("could not create log file for wash up test");

    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;

    let host_seed = nkeys::KeyPair::new_server();

    let mut up_cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "up",
            "--nats-port",
            "5893",
            "--dashboard-port",
            "5002",
            "-o",
            "json",
            "--detached",
            "--host-seed",
            &host_seed.seed().expect("Should have a seed for the host"),
        ])
        .kill_on_drop(true)
        .stdout(stdout)
        .spawn()
        .context("Could not spawn wash up process")?;

    let status = up_cmd
        .wait()
        .await
        .context("up command failed to complete")?;

    assert!(status.success(), "failed to complete up command");
    let out = read_to_string(&path).expect("could not read output of wash up");

    // Extract kill comamnd for later
    let (kill_cmd, _wasmcloud_log) = match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) => (v["kill_cmd"].to_owned(), v["wasmcloud_log"].to_owned()),
        Err(_e) => panic!("Unable to parse kill cmd from wash up output"),
    };

    // Wait for a single host to exis
    let host = wait_for_single_host(5893, Duration::from_secs(10), Duration::from_secs(1)).await?;

    let start_echo = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "start",
            "actor",
            "wasmcloud.azurecr.io/echo:0.3.4",
            "--ctl-port",
            "5893",
            "--timeout-ms",
            "10000", // Wait up to 10 seconds for slowpoke systems
        ])
        .output()
        .await
        .context(format!(
            "could not start echo actor on new host [{}]",
            host.id
        ))?;

    let stdout = String::from_utf8_lossy(&start_echo.stdout);
    let actor_start_output_rgx =
        Regex::new(RGX_ACTOR_START_MSG).expect("failed to create regular expression");
    assert!(
        actor_start_output_rgx.is_match(&stdout),
        "Did not find the correct output when starting actor.\n stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&start_echo.stderr)
    );

    let kill_cmd = kill_cmd.to_string();
    let (_wash, down) = kill_cmd.trim_matches('"').split_once(' ').unwrap();
    Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(vec![
            down,
            "--ctl-port",
            "5893",
            "--host-id",
            &host_seed.public_key(),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .context("Could not spawn wash down process")?;

    // Wait until the beam.smp process has finished and exited
    wait_for_no_hosts()
        .await
        .context("wasmcloud instance failed to exit cleanly (processes still left over)")?;

    remove_dir_all(dir).unwrap();
    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_up_can_stop_detached_host_serial() -> Result<()> {
    let dir = test_dir_with_subfolder("can_stop_wasmcloud");
    let path = dir.join("washup.log");
    let stdout = std::fs::File::create(&path).expect("could not create log file for wash up test");
    let nats_port: u16 = 5894;

    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;

    let host_seed = nkeys::KeyPair::new_server();

    let mut up_cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "up",
            "--nats-port",
            nats_port.to_string().as_ref(),
            "-o",
            "json",
            "--detached",
            "--host-seed",
            &host_seed.seed().expect("Should have a seed for the host"),
        ])
        .kill_on_drop(true)
        .stdout(stdout)
        .spawn()
        .context("Could not spawn wash up process")?;

    let status = up_cmd
        .wait()
        .await
        .context("up command failed to complete")?;

    assert!(status.success());
    let out = read_to_string(&path).expect("could not read output of wash up");

    let (kill_cmd, _wasmcloud_log) = match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) => (v["kill_cmd"].to_owned(), v["wasmcloud_log"].to_owned()),
        Err(_e) => panic!("Unable to parse kill cmd from wash up output"),
    };

    // Wait for a single host to exist
    wait_for_single_host(nats_port, Duration::from_secs(10), Duration::from_secs(1)).await?;

    // Stop the wash instance
    let kill_cmd = kill_cmd.to_string();
    let (_wash, down) = kill_cmd.trim_matches('"').split_once(' ').unwrap();
    Command::new(env!("CARGO_BIN_EXE_wash"))
        .args(vec![
            down,
            "--ctl-port",
            nats_port.to_string().as_ref(),
            "--host-id",
            &host_seed.public_key(),
        ])
        .output()
        .await
        .context("Could not spawn wash down process")?;

    // Wait until the beam.smp process has finished and exited
    wait_for_no_hosts()
        .await
        .context("wasmcloud instance failed to exit cleanly (processes still left over)")?;

    remove_dir_all(dir).unwrap();
    Ok(())
}

#[tokio::test]
#[serial]
async fn integration_up_doesnt_kill_unowned_nats_serial() -> Result<()> {
    let dir = test_dir_with_subfolder("doesnt_kill_unowned_nats");
    let path = dir.join("washup.log");
    let stdout = std::fs::File::create(&path).expect("could not create log file for wash up test");
    let nats_port: u16 = 5895;

    // Check that there are no beam.smp (wasmcloud instance) processes running
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;

    let mut nats = start_nats(5895, &dir).await?;

    let mut up_cmd = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "up",
            "--nats-port",
            nats_port.to_string().as_ref(),
            "--nats-connect-only",
            "-o",
            "json",
            "--detached",
        ])
        .kill_on_drop(true)
        .stdout(stdout)
        .spawn()
        .context("Could not spawn wash up process")?;

    let status = up_cmd
        .wait()
        .await
        .context("up command failed to complete")?;

    assert!(status.success());
    let out = read_to_string(&path).expect("could not read output of wash up");

    let (kill_cmd, _wasmcloud_log) = match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) => (v["kill_cmd"].to_owned(), v["wasmcloud_log"].to_owned()),
        Err(_e) => panic!("Unable to parse kill cmd from wash up output"),
    };

    // Wait for a single host to exist
    wait_for_single_host(nats_port, Duration::from_secs(10), Duration::from_secs(1)).await?;

    let kill_cmd = kill_cmd.to_string();
    let (_wash, down) = kill_cmd.trim_matches('"').split_once(' ').unwrap();
    Command::new(env!("CARGO_BIN_EXE_wash"))
        .kill_on_drop(true)
        .args(vec![down, "--ctl-port", nats_port.to_string().as_ref()])
        .output()
        .await
        .context("Could not spawn wash down process")?;

    // Check that there is exactly one nats-server running
    wait_for_nats_to_start()
        .await
        .context("nats process not running")?;

    nats.kill().await.map_err(|e| anyhow!(e))?;
    remove_dir_all(dir).unwrap();
    Ok(())
}
