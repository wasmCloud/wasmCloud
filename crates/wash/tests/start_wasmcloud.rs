use std::collections::HashMap;

use anyhow::Context;
use semver::Version;
use tempfile::tempdir;
use tokio::time::Duration;

use wash::lib::common::CommandGroupUsage;
use wash::lib::start::{
    ensure_nats_server, ensure_wasmcloud, ensure_wasmcloud_for_os_arch_pair, find_wasmcloud_binary,
    start_nats_server, start_wasmcloud_host, NatsConfig, NATS_SERVER_BINARY,
};

use wash::cli::config::NATS_SERVER_VERSION;

mod common;
use common::find_open_port;

const WASMCLOUD_VERSION: &str = "1.4.2";

#[tokio::test]
#[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
async fn can_download_wasmcloud_host() {
    let version = Version::parse(WASMCLOUD_VERSION).unwrap();
    let download_dir = tempdir().expect("Unable to create tempdir");
    let res = ensure_wasmcloud_for_os_arch_pair(&version, &download_dir)
        .await
        .expect("Should be able to download tarball");

    // Make sure we can find the binary and that it matches the path we got back from ensure
    assert_eq!(
        find_wasmcloud_binary(&download_dir, &version)
            .await
            .expect("Should have found installed wasmcloud"),
        res
    );

    // Just to triple check, make sure the paths actually exist
    assert!(
        download_dir
            .path()
            .join(format!("v{WASMCLOUD_VERSION}"))
            .exists(),
        "Directory should exist"
    );
}

#[tokio::test]
#[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
async fn can_download_and_start_wasmcloud() -> anyhow::Result<()> {
    let install_dir = tempdir().expect("Unable to create tempdir");

    // Install and start NATS server for this test
    let nats_port = find_open_port().await?;
    let nats_ws_port = find_open_port().await?;
    ensure_nats_server(NATS_SERVER_VERSION, &install_dir)
        .await
        .expect("Should be able to install NATS server");

    let mut config = NatsConfig::new_standalone("127.0.0.1", nats_port, None);
    config.websocket_port = nats_ws_port;
    let mut nats_child = start_nats_server(
        install_dir.path().join(NATS_SERVER_BINARY),
        std::process::Stdio::null(),
        config,
        CommandGroupUsage::UseParent,
    )
    .await
    .expect("Unable to start nats process");

    let wasmcloud_binary =
        ensure_wasmcloud(&Version::parse(WASMCLOUD_VERSION).unwrap(), &install_dir)
            .await
            .expect("Unable to ensure wasmcloud");

    let stderr_log_path = wasmcloud_binary
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("wasmcloud_stderr.log");
    let stderr_log_file = tokio::fs::File::create(&stderr_log_path)
        .await?
        .into_std()
        .await;
    let stdout_log_path = wasmcloud_binary
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("wasmcloud_stdout.log");
    let stdout_log_file = tokio::fs::File::create(&stdout_log_path)
        .await?
        .into_std()
        .await;

    let mut host_env = HashMap::new();
    host_env.insert("WASMCLOUD_RPC_PORT".to_string(), nats_port.to_string());
    host_env.insert("WASMCLOUD_CTL_PORT".to_string(), nats_port.to_string());
    let mut host_child = start_wasmcloud_host(
        &wasmcloud_binary,
        stdout_log_file,
        stderr_log_file,
        host_env,
    )
    .await
    .expect("Unable to start wasmcloud host");

    // Wait at most 10 seconds for wasmcloud to start
    println!("waiting for wasmcloud to start..");
    let startup_log_path = stderr_log_path.clone();
    tokio::time::timeout(Duration::from_secs(10), async move {
        loop {
            match tokio::fs::read_to_string(&startup_log_path).await {
                Ok(file_contents) if !file_contents.is_empty() => break,
                _ => {
                    println!("wasmCloud hasn't started up yet, waiting 1 second");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    })
    .await
    .context("failed to start wasmcloud (log path is missing)")?;

    // Wait for up to 15 seconds for the logs to contain expected lines
    println!("wasmCloud has started, waiting for expected startup logs...");
    let startup_log_path = stderr_log_path.clone();
    tokio::time::timeout(Duration::from_secs(15), async move {
        loop {
            match tokio::fs::read_to_string(&startup_log_path).await {
                Ok(file_contents) => {
                    if file_contents.contains("wasmCloud host started") {
                        // After wasmcloud says it's ready, it still requires some seconds to start up.
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        break;
                    }
                }
                _ => {
                    println!("no host startup logs in output yet, waiting 1 second");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    })
    .await
    .context("failed to start wasmcloud (logs did not contain expected content)")?;

    // We support multiple hosts, so this should work fine
    let mut host_env = HashMap::new();
    host_env.insert("WASMCLOUD_RPC_PORT".to_string(), nats_port.to_string());
    host_env.insert("WASMCLOUD_CTL_PORT".to_string(), nats_port.to_string());
    let mut child_res = start_wasmcloud_host(
        &wasmcloud_binary,
        std::process::Stdio::null(),
        std::process::Stdio::null(),
        host_env,
    )
    .await
    .expect("Unable to start host");
    child_res.kill().await?;

    host_child.kill().await?;
    nats_child.kill().await?;
    Ok(())
}
