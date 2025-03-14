use std::env;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};
use tokio::process::Command;
use tokio::time::{sleep, timeout, Duration};
use url::Url;
use wasmcloud_test_util::env::EnvVarGuard;

use super::{free_port, BackgroundServer};

pub async fn start_spire_agent(
    join_token: &str,
    server_url: Url,
    temp_dir: &Path,
) -> Result<(BackgroundServer, PathBuf, PathBuf)> {
    let agent_data_dir = temp_dir.join("data/agent");
    let api_socket_path = temp_dir.join("public/api.sock");
    let admin_socket_path = temp_dir.join("admin/api.sock");

    let _host_env = EnvVarGuard::set(
        "SPIRE_SERVER_HOST",
        server_url.host_str().unwrap_or("localhost"),
    );
    let _port_env = EnvVarGuard::set(
        "SPIRE_SERVER_PORT",
        server_url.port().unwrap_or(8081).to_string(),
    );
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

pub async fn start_spire_server(temp_dir: &Path) -> Result<(BackgroundServer, Url, PathBuf)> {
    let port = free_port().await?;
    let url = Url::parse(&format!("tcp://localhost:{port}"))
        .context("failed to parse SPIRE Server URL")?;

    let server_socket_path = temp_dir.join("private/api.sock");
    let server_data_dir = temp_dir.join("data/server");

    let _bind_port_env = EnvVarGuard::set("SERVER_BIND_PORT", port.to_string());
    let _socket_path_env = EnvVarGuard::set("SERVER_SOCKET_PATH", &server_socket_path);
    let _data_dir_env = EnvVarGuard::set("SERVER_DATA_DIR", server_data_dir);

    let server = BackgroundServer::spawn(
        Command::new(
            env::var("TEST_SPIRE_SERVER_BIN")
                .as_deref()
                .unwrap_or("spire-server"),
        )
        .args([
            "run",
            "-config",
            "./tests/fixtures/server.conf",
            "-expandEnv",
        ]),
    )
    .await
    .context("failed to start SPIRE Server")?;

    let socket_path = server_socket_path.display().to_string();
    timeout(Duration::from_secs(5), async move {
        loop {
            if let Ok(status) = Command::new(
                env::var("TEST_SPIRE_SERVER_BIN")
                    .as_deref()
                    .unwrap_or("spire-server"),
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
    .context("failed to call healthcheck on SPIRE Server")?;

    Ok((server, url, server_socket_path.clone()))
}

// Generates a join token used by the SPIRE Agent to establish it's identity
// with the SPIRE Server.
pub async fn generate_join_token(
    agent_spiffe_id: &str,
    spire_server_socket: &Path,
) -> anyhow::Result<String> {
    let output = Command::new(
        env::var("TEST_SPIRE_SERVER_BIN")
            .as_deref()
            .unwrap_or("spire-server"),
    )
    .args([
        "token",
        "generate",
        "-spiffeID",
        agent_spiffe_id,
        "-output",
        "json",
        "-ttl",
        "3600",
        "-socketPath",
        &spire_server_socket.display().to_string(),
    ])
    .output()
    .await
    .context("failed to generate SPIRE Agent join token")?;

    if !output.status.success() {
        bail!(
            "failed to generate SPIRE Agent join token, error: {}",
            String::from_utf8(output.stderr).unwrap_or_default()
        )
    }

    let join_token: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("should parse SPIRE Server join token response")?;

    Ok(join_token
        .get("value")
        .context("should find a 'value' field in the join token response")?
        .as_str()
        .context("should return join token 'value' field")?
        .to_string())
}

// Uses the SPIRE Server container to register the test workload under the
// local SPIRE Agent
pub async fn register_spiffe_workload(
    agent_spiffe_id: &str,
    workload_spiffe_id: &str,
    workload_selector: &str,
    spire_server_socket: &Path,
) -> anyhow::Result<()> {
    let output = Command::new(
        env::var("TEST_SPIRE_SERVER_BIN")
            .as_deref()
            .unwrap_or("spire-server"),
    )
    .args([
        "entry",
        "create",
        "-parentID",
        agent_spiffe_id,
        "-spiffeID",
        workload_spiffe_id,
        "-selector",
        workload_selector,
        "-socketPath",
        &spire_server_socket.display().to_string(),
    ])
    .output()
    .await
    .context("failed to register create SPIFFE entry for workload")?;

    if !output.status.success() {
        bail!(
            "failed to register create SPIFFE entry for workload, error: {}",
            String::from_utf8(output.stderr).unwrap_or_default()
        )
    }

    Ok(())
}

// Ensure that workloads can be fetched (within provided timeout), so that
// subsequent calls to the SPIRE Agent from the test workload and Auth Callout
// service succeed
pub async fn validate_workload_registration_within_timeout(
    agent_socket: &Path,
    timeout: Duration,
) -> anyhow::Result<()> {
    tokio::time::timeout(timeout, async move {
        loop {
            if let Ok(status) = tokio::process::Command::new(
                env::var("TEST_SPIRE_AGENT_BIN")
                    .as_deref()
                    .unwrap_or("spire-agent"),
            )
            .args([
                "api",
                "fetch",
                "x509",
                "-silent",
                "-socketPath",
                &agent_socket.display().to_string(),
            ])
            .status()
            .await
            {
                if status.success() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await?;
    Ok(())
}
