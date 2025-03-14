use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tokio::process::Command;
use tokio::time::{sleep, timeout, Duration};
use wasmcloud_test_util::env::EnvVarGuard;

use super::BackgroundServer;

pub async fn start_spire_agent(
    join_token: &str,
    server_url: &str,
    temp_dir: &Path,
) -> Result<(BackgroundServer, PathBuf, PathBuf)> {
    let url = url::Url::parse(server_url).context("failed to parse SPIRE Server url")?;

    let agent_data_dir = temp_dir.join("data/agent");
    let api_socket_path = temp_dir.join("public/api.sock");
    let admin_socket_path = temp_dir.join("admin/api.sock");

    let _host_env = EnvVarGuard::set("SPIRE_SERVER_HOST", url.host_str().unwrap_or("localhost"));
    let _port_env = EnvVarGuard::set("SPIRE_SERVER_PORT", url.port().unwrap_or(8081).to_string());
    let _data_dir_env = EnvVarGuard::set("AGENT_DATA_DIR", agent_data_dir);
    let _api_socket_env = EnvVarGuard::set("API_ENDPOINT_PATH", api_socket_path.clone());
    let _admin_socket_env = EnvVarGuard::set("ADMIN_ENDPOINT_PATH", admin_socket_path.clone());

    let agent = BackgroundServer::spawn(
        Command::new(
            env::var("TEST_SPIRE_AGENT_BIN")
                .as_deref()
                .unwrap_or("spire-agent"),
        )
        .args([
            "run",
            "-config",
            "./tests/fixtures/agent.conf",
            "-expandEnv",
            "-joinToken",
            join_token,
        ]),
    )
    .await
    .context("failed to start SPIRE Agent")?;

    let socket_path = api_socket_path.display().to_string();
    timeout(Duration::from_secs(5), async move {
        loop {
            if let Ok(status) = Command::new(
                env::var("TEST_SPIRE_AGENT_BIN")
                    .as_deref()
                    .unwrap_or("spire-agent"),
            )
            .args(["healthcheck", "-socketPath", &socket_path])
            .status()
            .await
            {
                if status.success() {
                    break;
                }
            }
            sleep(Duration::from_millis(250)).await;
        }
    })
    .await
    .context("failed to call healthcheck on SPIRE Agent")?;

    Ok((agent, api_socket_path, admin_socket_path))
}
