use regex::Regex;
use std::{
    fs::{read_to_string, remove_dir_all},
    path::PathBuf,
};

use anyhow::{anyhow, Result};
use common::{test_dir_with_subfolder, wash};
use sysinfo::{ProcessExt, SystemExt};
use tokio::process::Child;
use wash_lib::start::{ensure_nats_server, start_nats_server, NatsConfig};

mod common;

const RGX_ACTOR_START_MSG: &str = r"Actor \[(?P<actor_id>[^]]+)\] \(ref: \[(?P<actor_ref>[^]]+)\]\) started on host \[(?P<host_id>[^]]+)\]";

#[test]
fn integration_up_can_start_wasmcloud_and_actor() {
    let dir = test_dir_with_subfolder("can_start_wasmcloud");
    let path = dir.join("washup.log");
    let stdout = std::fs::File::create(&path).expect("could not create log file for wash up test");

    let host_seed = nkeys::KeyPair::new_server();

    let mut up_cmd = wash()
        .args([
            "up",
            "--nats-port",
            "5893",
            "-o",
            "json",
            "--detached",
            "--host-seed",
            &host_seed.seed().expect("Should have a seed for the host"),
        ])
        .stdout(stdout)
        .spawn()
        .expect("Could not spawn wash up process");

    let status = up_cmd.wait().expect("up command failed to complete");

    assert!(status.success());
    let out = read_to_string(&path).expect("could not read output of wash up");

    let (kill_cmd, _wasmcloud_log) = match serde_json::from_str::<serde_json::Value>(&out) {
        Ok(v) => (v["kill_cmd"].to_owned(), v["wasmcloud_log"].to_owned()),
        Err(_e) => panic!("Unable to parse kill cmd from wash up output"),
    };

    // Wait until the host starts, measured by trying to retrieve host inventory over NATS
    // Once this returns something other than a no responders, we know the host is ready for a ctl command
    let mut tries = 30;
    while tries >= 0 {
        let output = wash()
            .args(["ctl", "get", "inventory", &host_seed.public_key()])
            .output()
            .expect("expected command to finish");
        if output.stdout.is_empty() {
            tries -= 1;
            assert!(tries >= 0);
            std::thread::sleep(std::time::Duration::from_secs(1));
        } else {
            break;
        }
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
    let actor_start_output_rgx =
        Regex::new(RGX_ACTOR_START_MSG).expect("failed to create regular expression");
    assert!(
        actor_start_output_rgx.is_match(&stdout),
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

    // After `wash down` exits, sometimes Erlang things stick around for a few seconds
    std::thread::sleep(std::time::Duration::from_millis(5000));

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
