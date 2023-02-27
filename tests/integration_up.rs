use std::{
    fs::{read_to_string, remove_dir_all},
    path::PathBuf,
};

use anyhow::{anyhow, Result};
use common::{test_dir_with_subfolder, wash};
use serial_test::serial;
use sysinfo::{ProcessExt, SystemExt};
use tokio::process::Child;
use wash_lib::start::{ensure_nats_server, start_nats_server, NatsConfig};

mod common;

#[test]
#[serial]
fn integration_up_can_start_wasmcloud_and_actor() {
    let dir = test_dir_with_subfolder("can_start_wasmcloud");
    let path = dir.join("washup.log");
    let stdout = std::fs::File::create(&path).expect("could not create log file for wash up test");

    let mut up_cmd = wash()
        .args(["up", "--nats-port", "5893", "-o", "json", "--detached"])
        .stdout(stdout)
        .spawn()
        .expect("Could not spawn wash up process");

    let status = up_cmd.wait().expect("up command failed to complete");

    assert!(status.success());
    let out = read_to_string(&path).expect("could not read output of wash up");

    let (kill_cmd, wasmcloud_log) = match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) => (v["kill_cmd"].to_owned(), v["wasmcloud_log"].to_owned()),
        Err(_e) => panic!("Unable to parse kill cmd from wash up output"),
    };

    // Wait until the host starts
    let mut tries = 30;
    while !read_to_string(wasmcloud_log.to_string().trim_matches('"'))
        .expect("could not read output")
        .contains("Started wasmCloud OTP Host Runtime")
    {
        tries -= 1;
        assert!(tries >= 0);
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    let start_echo = wash()
        .args([
            "ctl",
            "start",
            "actor",
            "wasmcloud.azurecr.io/echo:0.3.4",
            "--ctl-port",
            "5893",
            "--timeout-ms",
            "10000", // Wait up to 10 seconds for slowpoke systems
        ])
        .output()
        .expect("could not start echo actor on new host");

    let stdout = String::from_utf8_lossy(&start_echo.stdout);
    assert!(
        stdout.contains("Actor wasmcloud.azurecr.io/echo:0.3.4 started on host N"),
        "Did not find the correct output when starting actor.\n stdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&start_echo.stderr)
    );

    let kill_cmd = kill_cmd.to_string();
    let (_wash, down) = kill_cmd.trim_matches('"').split_once(' ').unwrap();
    wash()
        .args(vec![down])
        .output()
        .expect("Could not spawn wash down process");

    remove_dir_all(dir).unwrap();
}

#[test]
#[serial]
fn can_stop_detached_host() {
    let dir = test_dir_with_subfolder("can_stop_wasmcloud");
    let path = dir.join("washup.log");
    let stdout = std::fs::File::create(&path).expect("could not create log file for wash up test");

    let mut up_cmd = wash()
        .args(["up", "--nats-port", "5894", "-o", "json", "--detached"])
        .stdout(stdout)
        .spawn()
        .expect("Could not spawn wash up process");

    let status = up_cmd.wait().expect("up command failed to complete");

    assert!(status.success());
    let out = read_to_string(&path).expect("could not read output of wash up");

    let (kill_cmd, wasmcloud_log) = match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) => (v["kill_cmd"].to_owned(), v["wasmcloud_log"].to_owned()),
        Err(_e) => panic!("Unable to parse kill cmd from wash up output"),
    };

    // Wait until the host starts
    let mut tries = 30;
    while !read_to_string(wasmcloud_log.to_string().trim_matches('"'))
        .expect("could not read output")
        .contains("Started wasmCloud OTP Host Runtime")
    {
        tries -= 1;
        assert!(tries >= 0);
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    let kill_cmd = kill_cmd.to_string();
    let (_wash, down) = kill_cmd.trim_matches('"').split_once(' ').unwrap();
    wash()
        .args(vec![down])
        .output()
        .expect("Could not spawn wash down process");

    // Check to see if process was removed
    let mut info = sysinfo::System::new_with_specifics(
        sysinfo::RefreshKind::new().with_processes(sysinfo::ProcessRefreshKind::new()),
    );

    info.refresh_processes();

    assert!(
        !info
            .processes()
            .values()
            .any(|p| p.exe().to_string_lossy().contains("beam.smp")),
        "No wasmcloud process should be running"
    );

    remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn doesnt_kill_unowned_nats() -> Result<()> {
    let dir = test_dir_with_subfolder("doesnt_kill_unowned_nats");
    let path = dir.join("washup.log");
    let stdout = std::fs::File::create(&path).expect("could not create log file for wash up test");

    let mut nats = start_nats(5895, &dir).await?;

    let mut up_cmd = wash()
        .args([
            "up",
            "--nats-port",
            "5895",
            "--nats-connect-only",
            "-o",
            "json",
            "--detached",
        ])
        .stdout(stdout)
        .spawn()
        .expect("Could not spawn wash up process");

    let status = up_cmd.wait().expect("up command failed to complete");

    assert!(status.success());
    let out = read_to_string(&path).expect("could not read output of wash up");

    let (kill_cmd, wasmcloud_log) = match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) => (v["kill_cmd"].to_owned(), v["wasmcloud_log"].to_owned()),
        Err(_e) => panic!("Unable to parse kill cmd from wash up output"),
    };

    // Wait until the host starts
    let mut tries = 30;
    while !read_to_string(wasmcloud_log.to_string().trim_matches('"'))
        .expect("could not read output")
        .contains("Started wasmCloud OTP Host Runtime")
    {
        tries -= 1;
        assert!(tries >= 0);
        std::thread::sleep(std::time::Duration::from_secs(1));
    }

    let kill_cmd = kill_cmd.to_string();
    let (_wash, down) = kill_cmd.trim_matches('"').split_once(' ').unwrap();
    wash()
        .args(vec![down])
        .output()
        .expect("Could not spawn wash down process");

    // Check to see if process was removed
    let mut info = sysinfo::System::new_with_specifics(
        sysinfo::RefreshKind::new().with_processes(sysinfo::ProcessRefreshKind::new()),
    );

    info.refresh_processes();

    assert!(
        info.processes()
            .values()
            .any(|p| p.exe().to_string_lossy().contains("nats-server")),
        "Nats server should still be running"
    );

    nats.kill().await.map_err(|e| anyhow!(e))?;
    remove_dir_all(dir).unwrap();
    Ok(())
}

async fn start_nats(port: u16, nats_install_dir: &PathBuf) -> Result<Child> {
    let nats_binary = ensure_nats_server("v2.8.4", nats_install_dir).await?;
    let config = NatsConfig::new_standalone("127.0.0.1", port, None);
    start_nats_server(nats_binary, std::process::Stdio::null(), config).await
}
