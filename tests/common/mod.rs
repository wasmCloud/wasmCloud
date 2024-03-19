use std::net::Ipv6Addr;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::Arc;

use anyhow::{anyhow, ensure, Context, Result};
use futures::StreamExt;
use hyper::header::HOST;
use hyper::Uri;
use tempfile::{NamedTempFile, TempDir};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::info;
use url::Url;

pub mod minio;
pub mod nats;
pub mod providers;
pub mod redis;
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

/// Copy a pre-built PAR file to a temporary location so that it can be used safely.
///
/// During CI, it is possible for a PAR to be written to during the process of a parallel test
/// triggering an file busy (EXTBSY) OS error. To avoid this, we copy the provider par
/// to a temporary directory and use that instead.
pub async fn copy_par(path: impl AsRef<Path>) -> Result<(Url, NamedTempFile)> {
    let provider_tmp = tempfile::Builder::new()
        .prefix("provider-tmp")
        .suffix(".par")
        .tempfile()
        .context("failed to make temp file for http provider")?;
    tokio::fs::copy(path.as_ref(), &provider_tmp)
        .await
        .context("failed to copy test par to new file")?;
    let provider_url =
        Url::from_file_path(provider_tmp.path()).expect("failed to construct provider ref");
    Ok((provider_url, provider_tmp))
}

/// Helper for serving HTTP requests via wrpc for testing. Will likely be subsumed once we have a
/// new http provider
pub async fn serve_incoming_http(
    wrpc_client: &Arc<wrpc_transport_nats::Client>,
    mut request: hyper::Request<hyper::body::Incoming>,
) -> anyhow::Result<
    hyper::Response<
        wrpc_interface_http::IncomingBody<
            wrpc_transport::IncomingInputStream,
            wrpc_interface_http::IncomingFields,
        >,
    >,
> {
    use wrpc_interface_http::IncomingHandler as _;

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
    let (response, tx, errors) = wrpc_client
        .invoke_handle_hyper(request)
        .await
        .context("failed to invoke `wrpc:http/incoming-handler.handle`")?;
    info!("await parameter transmit");
    tx.await.context("failed to transmit parameters")?;
    info!("await error collect");
    let errors: Vec<_> = errors.collect().await;
    assert!(errors.is_empty());
    info!("request served");
    response
}
