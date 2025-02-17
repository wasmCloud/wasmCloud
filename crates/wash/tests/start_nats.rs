use anyhow::Result;
use tempfile::tempdir;

use wash::cli::config::NATS_SERVER_VERSION;
use wash::lib::common::CommandGroupUsage;
use wash::lib::start::{ensure_nats_server, start_nats_server, NatsConfig, NATS_SERVER_BINARY};

mod common;
use common::find_open_port;

#[tokio::test]
#[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
async fn can_handle_missing_nats_version() -> Result<()> {
    let install_dir = tempdir().expect("Couldn't create tempdir");

    let res = ensure_nats_server("v300.22.1111223", &install_dir).await;
    assert!(res.is_err());

    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
async fn can_download_and_start_nats() -> Result<()> {
    let install_dir = tempdir().expect("Couldn't create tempdir");

    let res = ensure_nats_server(NATS_SERVER_VERSION, &install_dir).await;
    assert!(res.is_ok());

    let log_path = install_dir.path().join("nats.log");
    let log_file = tokio::fs::File::create(&log_path).await?.into_std().await;

    let nats_port = find_open_port().await?;
    let nats_ws_port = find_open_port().await?;
    let mut config = NatsConfig::new_standalone("127.0.0.1", nats_port, None);
    config.websocket_port = nats_ws_port;
    let child_res = start_nats_server(
        &install_dir.path().join(NATS_SERVER_BINARY),
        log_file,
        config,
        CommandGroupUsage::UseParent,
    )
    .await;
    assert!(child_res.is_ok());

    // Give NATS max 5 seconds to start up
    for _ in 0..4 {
        let log_contents = tokio::fs::read_to_string(&log_path).await?;
        if log_contents.is_empty() {
            println!("NATS server hasn't started up yet, waiting 1 second");
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
        } else {
            // Give just a little bit of time for the startup logs to flow in
            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

            assert!(log_contents.contains("Starting nats-server"));
            assert!(log_contents.contains("Starting JetStream"));
            assert!(log_contents.contains("Server is ready"));
            break;
        }
    }

    child_res.unwrap().kill().await?;
    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
async fn can_gracefully_fail_running_nats() -> Result<()> {
    let install_dir = tempdir().expect("Couldn't create tempdir");

    let res = ensure_nats_server(NATS_SERVER_VERSION, &install_dir).await;
    assert!(res.is_ok());

    let nats_port = find_open_port().await?;
    let nats_ws_port = find_open_port().await?;
    let mut config =
        NatsConfig::new_standalone("127.0.0.1", nats_port, Some("extender".to_string()));
    config.websocket_port = nats_ws_port;
    let nats_one = start_nats_server(
        &install_dir.path().join(NATS_SERVER_BINARY),
        std::process::Stdio::null(),
        config.clone(),
        CommandGroupUsage::UseParent,
    )
    .await;
    assert!(nats_one.is_ok());

    // Give NATS a few seconds to start up and listen
    tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
    let log_path = install_dir.path().join("nats.log");
    let log = std::fs::File::create(&log_path)?;
    let nats_two = start_nats_server(
        &install_dir.path().join(NATS_SERVER_BINARY),
        log,
        config,
        CommandGroupUsage::UseParent,
    )
    .await;
    assert!(nats_two.is_err());

    nats_one.unwrap().kill().await?;

    Ok(())
}
