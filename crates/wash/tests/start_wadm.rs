use wash::lib::common::CommandGroupUsage;
use wash::lib::start::{ensure_wadm, start_wadm, WadmConfig, WADM_BINARY, WADM_PID};

use anyhow::Result;
use tempfile::tempdir;

const WADM_VERSION: &str = "v0.18.0";

#[tokio::test]
#[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
async fn can_handle_missing_wadm_version() -> Result<()> {
    let install_dir = tempdir().expect("Unable to create tempdir");

    let major: u8 = 123;
    let minor: u8 = 52;
    let patch: u8 = 222;

    let res = ensure_wadm(&format!("v{major}.{minor}.{patch}"), &install_dir).await;
    assert!(res.is_err());

    Ok(())
}

#[tokio::test]
#[cfg_attr(not(can_reach_github_com), ignore = "github.com is not reachable")]
async fn can_download_and_start_wadm() -> Result<()> {
    let install_dir = tempdir().expect("Unable to create tempdir");

    let res = ensure_wadm(WADM_VERSION, &install_dir).await;
    assert!(res.is_ok());

    let log_path = install_dir.path().join("wadm.log");
    let log_file = tokio::fs::File::create(&log_path).await?.into_std().await;

    let config = WadmConfig {
        structured_logging: false,
        js_domain: None,
        nats_server_url: "nats://127.0.0.1:54321".to_string(),
        nats_credsfile: None,
    };

    let child_res = start_wadm(
        &install_dir,
        &install_dir.path().join(WADM_BINARY),
        log_file,
        Some(config),
        CommandGroupUsage::UseParent,
    )
    .await;
    assert!(child_res.is_ok());

    // Wait for process to exit since NATS couldn't connect
    assert!(child_res.unwrap().wait().await.is_ok());
    let log_contents = tokio::fs::read_to_string(&log_path).await?;
    // wadm couldn't connect to NATS but that's okay

    // Assert that the pid file get created in the expected state_dir,
    // which in this case is set to install_dir.
    let pid_path = install_dir.path().join(WADM_PID);
    assert!(tokio::fs::try_exists(pid_path).await?);

    // Different OS-es have different error codes, but all I care about is that wadm executed at all
    #[cfg(target_os = "macos")]
    assert!(log_contents.contains("Connection refused (os error 61)"));
    #[cfg(target_os = "linux")]
    assert!(log_contents.contains("Connection refused (os error 111)"));
    #[cfg(target_os = "windows")]
    assert!(log_contents.contains("No connection could be made because the target machine actively refused it. (os error 10061)"));

    Ok(())
}
