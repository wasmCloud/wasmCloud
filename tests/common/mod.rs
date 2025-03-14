use std::net::Ipv6Addr;
use std::process::ExitStatus;
use std::sync::Arc;

use anyhow::{anyhow, ensure, Context, Result};
use futures::StreamExt;
use hyper::header::HOST;
use hyper::Uri;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::info;

pub mod minio;
pub mod nats;
pub mod providers;
pub mod redis;
pub mod secrets;
pub mod spire;
pub mod vault;

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

/// Helper for serving HTTP requests via wrpc for testing. Will likely be subsumed once we have a
/// new http provider
pub async fn serve_incoming_http(
    wrpc_client: &Arc<wrpc_transport_nats::Client>,
    mut request: hyper::Request<hyper::body::Incoming>,
) -> anyhow::Result<hyper::Response<wrpc_interface_http::HttpBody>> {
    use wrpc_interface_http::InvokeIncomingHandler as _;

    let host = request.headers().get(HOST).expect("`host` header missing");
    let host = host
        .to_str()
        .expect("`host` header value is not a valid string");
    let path_and_query = request
        .uri()
        .path_and_query()
        .expect("`path_and_query` missing");
    let uri = Uri::builder()
        .scheme("http")
        .authority(host)
        .path_and_query(path_and_query.clone())
        .build()
        .expect("failed to build request URI");
    *request.uri_mut() = uri;
    info!(?request, "invoke `handle`");
    let (response, errs, io) = wrpc_client
        .invoke_handle_http(None, request)
        .await
        .context("failed to invoke `wrpc:http/incoming-handler.handle`")?;
    let response = response?;
    info!("await parameter transmit");
    if let Some(io) = io {
        io.await.context("failed to complete async I/O")?;
    }
    info!("await error collect");
    let errs: Vec<_> = errs.collect().await;
    assert!(errs.is_empty());
    info!("request served");
    Ok(response)
}
