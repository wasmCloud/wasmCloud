use std::fs::{read_to_string, remove_dir_all};
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use regex::Regex;
use semver::Version;
use serial_test::serial;
use tempfile::NamedTempFile;
use tokio::{process::Command, time::Duration};

mod common;
use common::{
    find_open_port, start_nats, test_dir_with_subfolder, wait_for_nats_to_start, wait_for_no_hosts,
    wait_for_single_host, TestWashInstance, HELLO_OCI_REF,
};
use wash::cli::config::WASMCLOUD_HOST_VERSION;

const RGX_COMPONENT_START_MSG: &str = r"Component \[(?P<component_id>[^]]+)\] \(ref: \[(?P<component_ref>[^]]+)\]\) started on host \[(?P<host_id>[^]]+)\]";

#[tokio::test]
#[serial]
#[cfg_attr(not(can_reach_ghcr_io), ignore = "ghcr.io is not reachable")]
async fn integration_up_can_start_wasmcloud_and_component_serial() -> Result<()> {
    let dir = test_dir_with_subfolder("can_start_wasmcloud");
    let path = dir.join("washup.log");
    let stdout = std::fs::File::create(&path).expect("could not create log file for wash up test");

    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;

    let host_seed = nkeys::KeyPair::new_server();

    let nats_port = find_open_port().await?;
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

    assert!(status.success(), "failed to complete up command");
    let out = read_to_string(&path).expect("could not read output of wash up");

    // Extract kill command for later
    let (kill_cmd, _wasmcloud_log) = match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) => (v["kill_cmd"].clone(), v["wasmcloud_log"].clone()),
        Err(_e) => panic!("Unable to parse kill cmd from wash up output"),
    };

    // Wait for a single host to exist
    let host =
        wait_for_single_host(nats_port, Duration::from_secs(10), Duration::from_secs(1)).await?;

    let start_echo = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "start",
            "component",
            HELLO_OCI_REF,
            "hello_component_id",
            "--ctl-port",
            nats_port.to_string().as_ref(),
            "--timeout-ms",
            "10000", // Wait up to 10 seconds for slowpoke systems
        ])
        .output()
        .await
        .context(format!(
            "could not start hello component on new host [{}]",
            host.id()
        ))?;

    let stdout = String::from_utf8_lossy(&start_echo.stdout);
    let component_start_output_rgx =
        Regex::new(RGX_COMPONENT_START_MSG).expect("failed to create regular expression");
    assert!(
        component_start_output_rgx.is_match(&stdout),
        "Did not find the correct output when starting component.\n stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&start_echo.stderr)
    );

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
        .kill_on_drop(true)
        .output()
        .await
        .context("Could not spawn wash down process")?;

    // Wait until the host process has finished and exited
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
    let nats_port: u16 = find_open_port().await?;

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
        Ok(v) => (v["kill_cmd"].clone(), v["wasmcloud_log"].clone()),
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

    // Wait until the host process has finished and exited
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
    let nats_port: u16 = find_open_port().await?;

    // Check that there are no host processes running
    wait_for_no_hosts()
        .await
        .context("unexpected wasmcloud instance(s) running")?;

    let mut nats = start_nats(nats_port, &dir).await?;

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
        Ok(v) => (v["kill_cmd"].clone(), v["wasmcloud_log"].clone()),
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

/// Ensure that wash up works with labels
#[tokio::test]
#[serial]
async fn integration_up_works_with_labels() -> Result<()> {
    let instance =
        TestWashInstance::create_with_extra_args(vec!["--label", "is-label-test=yes"]).await?;

    // Get host data, ensure we find the host with the right label
    let cmd_output = instance
        .get_hosts()
        .await
        .context("failed to call component")?;
    assert!(cmd_output.success, "call command succeeded");
    assert!(
        cmd_output
            .hosts
            .iter()
            .any(|h| h.labels().get("is-label-test").is_some_and(|v| v == "yes")),
        "a host is present which has the created label",
    );

    Ok(())
}

/// Ensure that wash up can start a new host with the new version of wasmcloud if a new patch is available
#[tokio::test]
#[serial]
async fn integration_up_works_with_new_patch_version_if_possible() -> Result<()> {
    // 1.0.2 is a sufficient version to test the latest is 1.0.4
    let a_previous_version = WASMCLOUD_HOST_VERSION.trim_start_matches("v");
    let instance: TestWashInstance = TestWashInstance::create().await?;

    let default_version = semver::Version::parse(a_previous_version)?;

    // Get host data, ensure we find the host with the right label
    let cmd_output = instance.get_hosts().await.context("failed to call hosts")?;

    assert!(cmd_output.success, "call command succeeded");
    let host = cmd_output.hosts.first();
    assert!(host.is_some(), "host is present");
    if let Some(host) = host {
        if let Some(version) = host.version() {
            let new_patched_version = semver::Version::parse(version)?;
            assert!(
                new_patched_version.major == default_version.major,
                "major version of host should not change"
            );
            assert!(
                new_patched_version.minor == default_version.minor,
                "minor version of host should not change"
            );
            assert!(
                new_patched_version.patch >= default_version.patch,
                "patch version cannot be smaller"
            );
        }
    }

    Ok(())
}

/// Ensure that wash up is starting a specific version
///  of wasmcloud host if wasmcloud parameter is specified
#[tokio::test]
#[serial]
async fn integration_up_works_with_specific_wasmcloud_host_version() -> Result<()> {
    let instance: TestWashInstance =
        TestWashInstance::create_with_extra_args(["--wasmcloud-version", "v1.0.4"]).await?;
    // Get host data, ensure we find the host with the right label
    let cmd_output = instance.get_hosts().await.context("failed to call hosts")?;

    assert!(cmd_output.success, "call command succeeded");
    let host = cmd_output.hosts.first();
    assert!(host.is_some(), "host is present");
    if let Some(host) = host {
        if let Some(version) = host.version() {
            assert_eq!(version, "1.0.4", "specified version is overwritten")
        }
    }

    Ok(())
}

/// Ensure that wash up can start a new host with a provided version of wadm
#[tokio::test]
#[serial]
async fn integration_up_works_with_specified_wadm_version() -> Result<()> {
    use wash::lib::config::{DOWNLOADS_DIR, WASH_DIR};
    use wash::lib::start::WADM_BINARY;
    // 0.12.0 is a sufficient version to test the latest is 0.12.2
    let previous_wadm_version = "v0.12.0";
    let home = etcetera::home_dir().context("no home directory found. Please set $HOME")?;

    let wadm_path = home
        .join(WASH_DIR)
        .join(DOWNLOADS_DIR)
        .join(WADM_BINARY)
        .canonicalize()
        .context("failed to canonicalize wadm binary path")?;
    let instance =
        TestWashInstance::create_with_extra_args(["--wadm-version", previous_wadm_version]).await?;
    let cmd_output = instance.get_hosts().await.context("failed to call hosts")?;
    assert!(cmd_output.success, "call command succeeded");
    let host = cmd_output.hosts.first();
    assert!(host.is_some(), "host is present");
    // NOTE: this assumes serial execution, otherwise the binary might be removed before the test
    let wadm_output = Command::new(wadm_path.clone())
        .args(["--version"])
        .output()
        .await
        .context("failed to run wadm --version")?;
    let wadm_version = String::from_utf8_lossy(&wadm_output.stdout);
    let wadm_version = wadm_version.trim_start_matches("wadm-cli ").trim();
    let Version {
        major,
        minor,
        patch,
        ..
    } = semver::Version::parse(wadm_version)?;
    let previous_version = semver::Version::parse(previous_wadm_version.trim_start_matches("v"))?;
    assert_eq!(
        major, previous_version.major,
        "major version should not change"
    );
    assert_eq!(
        minor, previous_version.minor,
        "minor version should not change"
    );
    assert_eq!(
        patch, previous_version.patch,
        "patch version should not change"
    );
    tokio::fs::remove_file(wadm_path.as_path()).await?;
    Ok(())
}

/// Ensure that wash up works with a provided WADM manifest
#[tokio::test]
#[serial]
async fn integration_up_works_with_wadm_manifest() -> Result<()> {
    let manifest_path = format!(
        "{}",
        PathBuf::from("./tests/fixtures/wadm/component-only.wadm.yaml")
            .canonicalize()?
            .display()
    );

    let instance =
        TestWashInstance::create_with_extra_args(vec!["--wadm-manifest", manifest_path.as_ref()])
            .await?;

    assert!(instance
        .deployed_wadm_manifest_path
        .as_ref()
        .is_some_and(|v| *v == manifest_path));

    Ok(())
}

/// Ensure that wash up works with a custom log file
#[tokio::test]
#[serial]
async fn integration_up_works_with_custom_log_file() -> Result<()> {
    let tmp = NamedTempFile::new().context("failed to create temporary log file")?;
    let tmp_path = format!("{}", tmp.path().display());

    let _instance =
        TestWashInstance::create_with_extra_args(vec!["--host-log-path", &tmp_path]).await?;

    // Ensure that the log file is and has zzero
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if tokio::fs::metadata(tmp.path())
                .await
                .is_ok_and(|metadata| metadata.len() > 0)
            {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;
        }
    })
    .await
    .with_context(|| format!("timed out waiting for non-empty log path [{tmp_path}]",))?;

    Ok(())
}
