use std::net::Ipv6Addr;
use std::process::ExitStatus;

use anyhow::{anyhow, ensure, Context, Result};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub mod nats;

/// Create a temporary directory
pub fn tempdir() -> Result<TempDir> {
    tempfile::tempdir().context("failed to create temporary directory")
}

/// Retrieve a free port to use from the OS
pub async fn free_port() -> Result<u16> {
    TcpListener::bind((Ipv6Addr::LOCALHOST, 0))
        .await
        .context("failed to start TCP listener")?
        .local_addr()
        .context("failed to query listener local address")
        .map(|v| v.port())
}

/// Generic utility for starting a background process properly with `tokio::spawn`
pub struct BackgroundServer {
    handle: JoinHandle<Result<ExitStatus>>,
    stop_tx: oneshot::Sender<()>,
}

impl BackgroundServer {
    /// Spawn a [`Command`] that is server-like, running it as a utility until a `()` is sent on the returned channel
    /// to trigger killing the subprocess process
    pub async fn spawn(cmd: &mut Command) -> Result<Self> {
        let mut child = cmd
            .kill_on_drop(true)
            .spawn()
            .context("failed to spawn child")?;
        let (stop_tx, stop_rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            tokio::select!(
                res = stop_rx => {
                    res.context("failed to wait for shutdown")?;
                    child.kill().await.context("failed to kill child")?;
                    child.wait().await
                }
                status = child.wait() => {
                    status
                }
            )
            .context("failed to wait for child")
        });
        Ok(Self { handle, stop_tx })
    }

    /// Stop the server, provided the relevant join handle and [`oneshot::Sender`] on which to send the stop
    pub async fn stop(self) -> Result<()> {
        self.stop_tx
            .send(())
            .map_err(|()| anyhow!("failed to send stop"))?;
        let status = self
            .handle
            .await
            .context("failed to wait for server to exit")?
            .context("server failed to exit")?;
        ensure!(status.code().is_none());
        Ok(())
    }
}
