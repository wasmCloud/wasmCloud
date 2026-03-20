//! Integration tests for `wash oci push` / `wash oci pull` round-trip.
//!
//! Requires Docker and the `OCI_INTEGRATION_TESTS` env var to be set.
//! Skips gracefully otherwise.

use std::time::Duration;

use anyhow::{Context, Result};
use tempfile::TempDir;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use tokio::time::timeout;
use wash::cli::CliContext;
use wash::cli::oci::{PullCommand, PushCommand};

/// Start a local OCI registry container (distribution/distribution) on a random host port.
async fn start_registry() -> Result<(ContainerAsync<GenericImage>, u16)> {
    let container = GenericImage::new("distribution/distribution", "edge")
        .with_exposed_port(5000.into())
        .with_wait_for(testcontainers::core::WaitFor::message_on_stderr(
            "listening on",
        ))
        .start()
        .await
        .context("failed to start registry container")?;

    let port = container
        .get_host_port_ipv4(5000)
        .await
        .context("failed to get mapped port")?;

    Ok((container, port))
}

#[tokio::test]
async fn oci_push_pull_round_trip() -> Result<()> {
    // Gate on env var — matches pattern from wash-runtime OCI tests
    if std::env::var("OCI_INTEGRATION_TESTS")
        .unwrap_or_default()
        .is_empty()
    {
        eprintln!("Skipping OCI integration test (set OCI_INTEGRATION_TESTS=1 to enable)");
        return Ok(());
    }

    // Start local registry
    let (_container, port) = start_registry().await?;
    let reference = format!("localhost:{port}/test/wash-e2e:v1");

    // Create a minimal valid wasm component
    let component_bytes = wat::parse_str("(component)").context("failed to parse WAT")?;

    // Write component to a temp file
    let temp = TempDir::new().context("failed to create temp dir")?;
    let push_path = temp.path().join("push.wasm");
    tokio::fs::write(&push_path, &component_bytes)
        .await
        .context("failed to write component to temp file")?;

    // Build CLI context
    let ctx = CliContext::builder()
        .non_interactive(true)
        .project_dir(temp.path().to_path_buf())
        .build()
        .await
        .context("failed to create CLI context")?;

    // Push
    let push_cmd = PushCommand {
        reference: reference.clone(),
        component_path: push_path,
        insecure: true,
        user: None,
        password: None,
    };
    let push_result = timeout(Duration::from_secs(30), push_cmd.handle(&ctx))
        .await
        .context("push timed out")?
        .context("push failed")?;

    assert!(
        push_result.is_success(),
        "push should succeed: {push_result:?}"
    );

    let push_digest = push_result
        .json()
        .and_then(|j| j.get("digest"))
        .and_then(|d| d.as_str())
        .context("push result missing digest")?
        .to_string();

    // Pull to a different path
    let pull_path = temp.path().join("pulled.wasm");
    let pull_cmd = PullCommand {
        reference: reference.clone(),
        component_path: pull_path.clone(),
        insecure: true,
        user: None,
        password: None,
    };
    let pull_result = timeout(Duration::from_secs(30), pull_cmd.handle(&ctx))
        .await
        .context("pull timed out")?
        .context("pull failed")?;

    assert!(
        pull_result.is_success(),
        "pull should succeed: {pull_result:?}"
    );

    let pull_digest = pull_result
        .json()
        .and_then(|j| j.get("digest"))
        .and_then(|d| d.as_str())
        .context("pull result missing digest")?
        .to_string();

    // Verify round-trip integrity
    let pulled_bytes = tokio::fs::read(&pull_path)
        .await
        .context("failed to read pulled component")?;
    assert_eq!(
        component_bytes, pulled_bytes,
        "pulled bytes should match original"
    );

    // Verify digests match
    assert_eq!(
        push_digest, pull_digest,
        "push and pull digests should match"
    );

    Ok(())
}
