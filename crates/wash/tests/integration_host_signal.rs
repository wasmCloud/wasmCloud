//! Pins what a signal does to `wash host`, on both sides of the point where
//! there is something to shut down.
//!
//! Once the host is running the signal has to reach the graceful path — drain
//! in-flight commands, stop the host, unbind its plugins — rather than killing
//! the process where it stands. Kubernetes sends `SIGTERM`; a terminal sends
//! `SIGINT`. Both are covered, because they took different routes to the same
//! failure: `SIGTERM` had no handler at all, and `SIGINT` had one that exited
//! before the shutdown it had just started could finish.
//!
//! While the host is still starting there is nothing to shut down, and the
//! signal has to end the process instead. Holding it until startup finishes is
//! the other way to get this wrong: a host part-way through an image pull would
//! ignore every Ctrl-C it was sent.
//!
//! The running case needs Docker (NATS) and is `#[ignore]`d; the startup case
//! needs nothing but the binary.

#![cfg(unix)]

use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// 128 plus the signal number: what a process exits with when it leaves on the
/// signal it was given rather than running a shutdown.
const INTERRUPTED: i32 = 130;
const TERMINATED: i32 = 143;

/// A `wash host` under test, with its log collected as it runs.
struct HostProcess {
    child: Child,
    log: tokio::task::JoinHandle<std::io::Result<String>>,
    started: oneshot::Receiver<()>,
}

impl HostProcess {
    /// Starts a host against `nats_url`, with a home and working directory of
    /// its own and nothing inherited: the host reads `RUST_LOG` for the log
    /// lines asserted here and `WASH_*` for half its flags, so whatever is set
    /// on the machine running the test would otherwise decide what it sees.
    fn spawn(nats_url: &str, home: &Path) -> Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_wash"))
            .args([
                "host",
                "--host-group",
                "signal-test",
                "--scheduler-nats-url",
                nats_url,
                "--data-nats-url",
                nats_url,
            ])
            .env_clear()
            .current_dir(home)
            .env("HOME", home)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("failed to spawn wash host")?;

        let stderr = child.stderr.take().context("stderr was piped")?;
        let (started_tx, started) = oneshot::channel();
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

        Ok(Self {
            child,
            log,
            started,
        })
    }

    /// Signals the host the way anything outside this process would — `kill(1)`
    /// rather than a Rust-side handle, so it sees exactly what kubelet or a
    /// terminal sends it — and reports how it went out.
    async fn signalled_with(mut self, signal: &str) -> Result<(Option<i32>, String)> {
        let pid = self.child.id().context("host has already exited")?;
        let killed = std::process::Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(pid.to_string())
            .status()
            .context("failed to run kill")?;
        if !killed.success() {
            bail!("kill -{signal} {pid} failed: {killed}");
        }

        let status = timeout(SHUTDOWN_TIMEOUT, self.child.wait())
            .await
            .with_context(|| format!("host still running {SHUTDOWN_TIMEOUT:?} after SIG{signal}"))?
            .context("failed to wait for the host to exit")?;

        Ok((status.code(), self.log.await??))
    }
}

/// Runs a host on a throwaway NATS, waits until it is up, and signals it.
async fn running_host_signalled_with(signal: &str) -> Result<(Option<i32>, String)> {
    let nats = GenericImage::new("nats", "2.12.8-alpine")
        .with_exposed_port(4222.tcp())
        .with_wait_for(WaitFor::message_on_stderr("Server is ready"))
        .with_cmd(["-js"])
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("failed to start nats: {e}"))?;
    let nats_url = format!("nats://127.0.0.1:{}", nats.get_host_port_ipv4(4222).await?);

    let home = TempDir::new().context("failed to create temp home")?;
    let mut host = HostProcess::spawn(&nats_url, home.path())?;

    if timeout(STARTUP_TIMEOUT, &mut host.started).await.is_err() {
        bail!("host never started: {}", host.log.await??);
    }
    host.signalled_with(signal).await
}

/// Parks a host in its startup and signals it there.
async fn starting_host_signalled_with(signal: &str) -> Result<(Option<i32>, String)> {
    let nats = StalledNats::bind().await?;
    let home = TempDir::new().context("failed to create temp home")?;
    let host = HostProcess::spawn(&nats.url(), home.path())?;

    // The host connects to its scheduler NATS after arming its signals and long
    // before it has a host to stop, so a connection here puts the signal below
    // squarely inside the startup window.
    let reached = timeout(STARTUP_TIMEOUT, async {
        while nats.reached() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if reached.is_err() {
        bail!("host never reached its NATS: {}", host.log.await??);
    }

    host.signalled_with(signal).await
}

/// A listener that completes the TCP connect and then says nothing, so the
/// host's NATS handshake hangs and it stays in startup.
struct StalledNats {
    addr: SocketAddr,
    reached: Arc<AtomicUsize>,
    accept: tokio::task::JoinHandle<()>,
}

impl StalledNats {
    async fn bind() -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind stalling NATS")?;
        let addr = listener
            .local_addr()
            .context("failed to read stalling NATS address")?;
        let reached = Arc::new(AtomicUsize::new(0));
        let accept = tokio::spawn({
            let reached = Arc::clone(&reached);
            async move {
                // Accepted and held, never answered.
                let mut accepted = Vec::new();
                while let Ok((stream, _)) = listener.accept().await {
                    reached.fetch_add(1, Ordering::SeqCst);
                    accepted.push(stream);
                }
            }
        });
        Ok(Self {
            addr,
            reached,
            accept,
        })
    }

    fn url(&self) -> String {
        format!("nats://{}", self.addr)
    }

    /// Connections accepted so far: one per attempt the host made to reach it.
    fn reached(&self) -> usize {
        self.reached.load(Ordering::SeqCst)
    }
}

impl Drop for StalledNats {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

/// The signal Kubernetes sends a terminating pod. Without a handler for it the
/// process takes the default disposition and dies where it stands, which is
/// every plugin left bound behind it.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn sigterm_runs_the_graceful_shutdown() -> Result<()> {
    let (code, log) = running_host_signalled_with("TERM").await?;
    assert_eq!(code, Some(0), "host did not exit cleanly on SIGTERM: {log}");
    assert!(log.contains(STOPPING), "host skipped its shutdown: {log}");
    Ok(())
}

/// The same path from a terminal. This one exits cleanly only as long as no
/// process-wide handler exits ahead of the shutdown it starts.
#[tokio::test]
#[ignore = "requires Docker (NATS); run with `cargo test --include-ignored`"]
async fn sigint_runs_the_graceful_shutdown() -> Result<()> {
    let (code, log) = running_host_signalled_with("INT").await?;
    assert_eq!(code, Some(0), "host did not exit cleanly on SIGINT: {log}");
    assert!(log.contains(STOPPING), "host skipped its shutdown: {log}");
    Ok(())
}

/// A pod terminated while its host is still starting — stuck on an unreachable
/// NATS here, an image pull in a cluster. There is nothing to shut down yet, so
/// the signal has to end the process: the exit code says the host answered it,
/// where dying by the signal would mean nothing was listening, and a test that
/// times out here means the signal was recorded and then ignored.
#[tokio::test]
async fn sigterm_during_startup_ends_the_process() -> Result<()> {
    let (code, log) = starting_host_signalled_with("TERM").await?;
    assert_eq!(
        code,
        Some(TERMINATED),
        "host did not leave on SIGTERM during startup: {log}"
    );
    assert!(
        !log.contains(STOPPING),
        "host ran a shutdown for a host it had not started: {log}"
    );
    Ok(())
}

/// The same, for the Ctrl-C of someone who has waited long enough on a pull.
#[tokio::test]
async fn sigint_during_startup_ends_the_process() -> Result<()> {
    let (code, log) = starting_host_signalled_with("INT").await?;
    assert_eq!(
        code,
        Some(INTERRUPTED),
        "host did not leave on SIGINT during startup: {log}"
    );
    assert!(
        !log.contains(STOPPING),
        "host ran a shutdown for a host it had not started: {log}"
    );
    Ok(())
}
