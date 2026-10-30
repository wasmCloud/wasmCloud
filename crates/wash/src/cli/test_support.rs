//! Fixtures shared by the command tests.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result, ensure};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::TcpListener,
    task::JoinHandle,
    time::timeout,
};

use super::{CliContext, DirectoryStrategy};

const WAIT: Duration = Duration::from_secs(10);

/// Keeps every configuration, data and cache path under one test directory.
#[derive(Debug)]
struct TestDirectories(PathBuf);

impl DirectoryStrategy for TestDirectories {
    fn home_dir(&self) -> &Path {
        &self.0
    }
    fn config_dir(&self) -> PathBuf {
        self.0.join("config")
    }
    fn data_dir(&self) -> PathBuf {
        self.0.join("data")
    }
    fn cache_dir(&self) -> PathBuf {
        self.0.join("cache")
    }
    fn state_dir(&self) -> Option<PathBuf> {
        None
    }
    fn runtime_dir(&self) -> Option<PathBuf> {
        None
    }
}

pub(crate) fn context(root: &Path) -> CliContext {
    CliContext {
        app_strategy: Arc::new(TestDirectories(root.into())),
        non_interactive: true,
        config: None,
        project_dir: root.into(),
        original_working_dir: root.into(),
        meters: Default::default(),
    }
}

/// One canned HTTP response. `content_length` may disagree with `body` to
/// simulate a truncated transfer.
pub(crate) struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_length: usize,
}

impl Response {
    pub(crate) fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        let body = body.into();
        Self {
            status,
            content_length: body.len(),
            body,
        }
    }
}

/// The request line and headers of one request the fixture answered.
pub(crate) struct Request {
    pub target: String,
    pub headers: HeaderMap,
}

/// A loopback HTTP/1.1 server that answers one connection per response, in
/// order, and records what it was asked.
pub(crate) struct HttpFixture {
    pub address: SocketAddr,
    task: Option<JoinHandle<Result<Vec<Request>>>>,
}

impl HttpFixture {
    pub(crate) async fn start(responses: Vec<Response>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut socket, _) = timeout(WAIT, listener.accept()).await??;
                // Drain the request headers before closing to avoid a TCP reset.
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    ensure!(request.len() < 64 * 1024, "request headers too large");
                    request.push(timeout(WAIT, socket.read_u8()).await??);
                }
                let request = String::from_utf8(request)?;
                let mut lines = request.lines();
                let target = lines.next().context("missing request line")?.to_owned();
                let mut headers = HeaderMap::new();
                for line in lines.filter(|line| !line.is_empty()) {
                    let (name, value) = line.split_once(':').context("invalid header")?;
                    headers.insert(
                        HeaderName::from_bytes(name.as_bytes())?,
                        HeaderValue::from_str(value.trim())?,
                    );
                }
                requests.push(Request { target, headers });
                let Response {
                    status,
                    body,
                    content_length,
                } = response;
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 {status} Test\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await?;
                socket.write_all(&body).await?;
            }
            Ok(requests)
        });
        Ok(Self {
            address,
            task: Some(task),
        })
    }

    /// Waits for every response to be served and returns the requests.
    pub(crate) async fn finish(&mut self) -> Result<Vec<Request>> {
        let task = self.task.take().context("fixture already finished")?;
        timeout(WAIT, task).await??
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
