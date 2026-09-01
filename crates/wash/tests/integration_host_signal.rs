//! Pins the shutdown a terminating `wash host` runs: the signal reaches the
//! graceful path — drain in-flight commands, stop the host, unbind its plugins
//! — rather than killing the process where it stands.
//!
//! Kubernetes sends `SIGTERM`; a terminal sends `SIGINT`. Both are tested,
//! because they took different routes to the same failure: `SIGTERM` had no
//! handler at all, and `SIGINT` had one that exited before the shutdown it
//! had just started could finish.
//!
//! Requires Docker (NATS); marked `#[ignore]`, run with `cargo test --include-ignored`.

#![cfg(unix)]

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tempfile::TempDir;
use testcontainers::runners::AsyncRunner;
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
};
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use tokio::time::timeout;

/// Long enough for a debug-build host to come up on a loaded CI runner.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);
/// Comfortably past the host's own 5s command drain.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// What the host logs on the graceful path, after the signal and before
/// `host.stop()`.
const STOPPING: &str = "Stopping host...";

/// Runs `wash host` against a throwaway NATS, signals it with `signal`, and
/// returns its exit code and log once it is gone.
async fn host_signalled_with(signal: &str) -> Result<(Option<i32>, String)> {
    let nats = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start nats: {e}"))?;
    let nats_url = format!("nats://127.0.0.1:{}", nats.get_host_port_ipv4(4222).await?);

    // A home and working directory of its own, and nothing inherited: the
    // host reads `RUST_LOG` for the log lines asserted below and `WASH_*` for
    // half its flags, so whatever is set on the machine running the test would
    // otherwise decide what this test sees.
    let home = TempDir::new().context("failed to create temp home")?;
    let mut child = Command::new(env!("CARGO_BIN_EXE_wash"))
        .args([
            "host",
            "--host-group",
            "signal-test",
            "--scheduler-nats-url",
            &nats_url,
            "--data-nats-url",
            &nats_url,
        ])
        .env_clear()
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("RUST_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("failed to spawn wash host")?;

    let stderr = child.stderr.take().context("stderr was piped")?;
    let (started_tx, started_rx) = oneshot::channel();
    let log = tokio::spawn(async move {
        let mut started_tx = Some(started_tx);
        let mut lines = BufReader::new(stderr).lines();
        let mut log = String::new();
        while let Some(line) = lines.next_line().await? {
            if line.contains("Host started")
                && let Some(tx) = started_tx.take()
            {
                let _ = tx.send(());
            }
            log.push_str(&line);
            log.push('\n');
        }
        Ok::<_, std::io::Error>(log)
    });

    if timeout(STARTUP_TIMEOUT, started_rx).await.is_err() {
        let _ = child.kill().await;
        bail!("host never started: {}", log.await??);
    }

    signal_child(&child, signal)?;
    let status = timeout(SHUTDOWN_TIMEOUT, child.wait())
        .await
        .with_context(|| format!("host still running {SHUTDOWN_TIMEOUT:?} after SIG{signal}"))?
        .context("failed to wait for the host to exit")?;

    Ok((status.code(), log.await??))
}

/// Signals the host the way anything outside this process would — `kill(1)`
/// rather than a Rust-side handle, so the process sees exactly what kubelet or
/// a terminal sends it.
fn signal_child(child: &Child, signal: &str) -> Result<()> {
    let pid = child.id().context("host has already exited")?;
    let killed = std::process::Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()
        .context("failed to run kill")?;
    if !killed.success() {
        bail!("kill -{signal} {pid} failed: {killed}");
    }
    Ok(())
}

/// The signal Kubernetes sends a terminating pod. Without a handler for it the
/// process takes the default disposition and dies where it stands, which is
/// every plugin left bound behind it.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn sigterm_runs_the_graceful_shutdown() -> Result<()> {
    let (code, log) = host_signalled_with("TERM").await?;
    assert_eq!(code, Some(0), "host did not exit cleanly on SIGTERM: {log}");
    assert!(log.contains(STOPPING), "host skipped its shutdown: {log}");
    Ok(())
}

/// The same path from a terminal. This one exits cleanly only as long as no
/// process-wide handler exits ahead of the shutdown it starts.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn sigint_runs_the_graceful_shutdown() -> Result<()> {
    let (code, log) = host_signalled_with("INT").await?;
    assert_eq!(code, Some(0), "host did not exit cleanly on SIGINT: {log}");
    assert!(log.contains(STOPPING), "host skipped its shutdown: {log}");
    Ok(())
}
