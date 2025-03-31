//! Common types and utilities for HTTP client providers.
//!
//! This module provides a reusable connection pooling implementation that can be used
//! by both the internal and external wasmCloudHTTP client providers. It manages separate
//! pools for HTTP and HTTPS connections, allowing for efficient connection reuse.

use core::error::Error;
use core::ops::{Deref, DerefMut};
use core::time::Duration;
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock};
use std::time::Instant;
use tokio::join;
use tokio::net::{TcpStream, ToSocketAddrs};
use tokio::sync::{Mutex, RwLock};
use tokio::task::{AbortHandle, JoinSet};
use tracing::{trace, warn};

use wrpc_interface_http::bindings::{
    wasi::http::types::DnsErrorPayload, wrpc::http::types::ErrorCode,
};

// adapted from https://github.com/hyperium/hyper-util/blob/46826ea75836852fac53ff075a12cba7e290946e/src/client/legacy/client.rs#L1004
/// Default duration after which idle connections are closed.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Default User-Agent header value for HTTP requests.
pub const DEFAULT_USER_AGENT: &str =
    concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// Default HTTP connection timeout.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(600);

/// Default first byte timeout for HTTP connections (causingHTTP 503 errors).
pub const DEFAULT_FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(600);

/// Configuration key to control whether to load the system's native certificate store.
/// When set to "true" (default), the provider will use the operating system's trusted certificates.
pub const LOAD_NATIVE_CERTS: &str = "load_native_certs";

/// Configuration key to control whether to load the webpki certificate store.
/// When set to "true", the provider will use the Mozilla-curated certificate store bundled with webpki-roots.
pub const LOAD_WEBPKI_CERTS: &str = "load_webpki_certs";

/// Configuration key to specify a custom TLS certificate CA bundle file.
/// When provided, the provider will load certificates from this file in addition to other sources.
pub const SSL_CERTS_FILE: &str = "ssl_certs_file";

/// Instant used as the "zero" `last_seen` value.
/// Provides a consistent starting point for measuring connection age.
pub static ZERO_INSTANT: LazyLock<Instant> = LazyLock::new(Instant::now);

/// Represents a connection to a remote HTTP server that can be reused across multiple requests.
/// Includes the actual sender for making requests, a handle to abort the connection task,
/// and a timestamp for tracking when the connection was last used for connection pooling.
#[derive(Clone, Debug)]
pub struct PooledConn<T> {
    /// The HTTP sender for making requests
    pub sender: T,
    /// Handle to abort the connection task when no longer needed
    pub abort: AbortHandle,
    /// Timestamp of when this connection was last used
    pub last_seen: Instant,
}

impl<T> Deref for PooledConn<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.sender
    }
}

impl<T> DerefMut for PooledConn<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sender
    }
}

impl<T: PartialEq> PartialEq for PooledConn<T> {
    fn eq(
        &self,
        Self {
            sender,
            abort,
            last_seen,
        }: &Self,
    ) -> bool {
        self.sender == *sender && self.abort.id() == abort.id() && self.last_seen == *last_seen
    }
}

impl<T> Drop for PooledConn<T> {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

impl<T> PooledConn<T> {
    /// Creates a new pooled connection.
    ///
    /// # Arguments
    ///
    /// * `sender` - The HTTP sender for making requests
    /// * `abort` - Handle to abort the connection task when no longer needed
    pub fn new(sender: T, abort: AbortHandle) -> Self {
        Self {
            sender,
            abort,
            last_seen: *ZERO_INSTANT,
        }
    }
}

/// Type alias for the connection pool's internal storage structure.
/// Maps authority strings (e.g., "example.com:443") to queues of pooled connections
/// for efficient connection reuse.
pub type ConnPoolTable<T> =
    RwLock<HashMap<Box<str>, std::sync::Mutex<VecDeque<PooledConn<http1::SendRequest<T>>>>>>;

/// Manages separate pools for HTTP and HTTPS connections, allowing for efficient
/// connection reuse based on the target authority.
///
/// The connection pool maintains separate tables for HTTP and HTTPS connections,
/// indexed by authority (host:port). Each entry contains a queue of pooled connections
/// that can be reused for subsequent requests to the same authority.
#[derive(Debug)]
pub struct ConnPool<T> {
    /// Pool of HTTP connections indexed by authority
    pub http: Arc<ConnPoolTable<T>>,
    /// Pool of HTTPS connections indexed by authority
    pub https: Arc<ConnPoolTable<T>>,
    /// Background tasks for connection management
    pub tasks: Arc<Mutex<JoinSet<()>>>,
}

/// Default implementation for the connection pool, creating a new instance with
/// empty HTTP and HTTPS pools and a join set for managing tasks.
impl<T> Default for ConnPool<T> {
    fn default() -> Self {
        Self {
            http: Arc::default(),
            https: Arc::default(),
            tasks: Arc::default(),
        }
    }
}

impl<T> Clone for ConnPool<T> {
    fn clone(&self) -> Self {
        Self {
            http: self.http.clone(),
            https: self.https.clone(),
            tasks: self.tasks.clone(),
        }
    }
}

/// Evicts connections from the pool that have been idle for longer than the cutoff time.
///
/// # Arguments
///
/// * `cutoff` - Connections with a last_seen timestamp before this will be evicted
/// * `conns` - The connection pool to evict from
pub fn evict_conns<T>(
    cutoff: Instant,
    conns: &mut HashMap<Box<str>, std::sync::Mutex<VecDeque<PooledConn<T>>>>,
) {
    trace!(target: "http_client::evict", ?cutoff, total_authorities=conns.len(), "evicting connections older than cutoff");
    let mut total_evicted = 0;
    conns.retain(|authority, conns| {
        let Ok(conns) = conns.get_mut() else {
            trace!(target: "http_client::evict", %authority, "skipping locked connection pool");
            return true;
        };
        let total_conns = conns.len();
        let idx = conns.partition_point(|&PooledConn { last_seen, .. }| last_seen <= cutoff);
        if idx == conns.len() {
            trace!(target: "http_client::evict", %authority, evicted=total_conns, "evicting all connections");
            total_evicted += total_conns;
            false
        } else if idx == 0 {
            trace!(target: "http_client::evict", %authority, total=total_conns, "no connections to evict");
            true
        } else {
            trace!(target: "http_client::evict", %authority, evicted=idx, remaining=(total_conns - idx), "partially evicting connections");
            conns.rotate_left(idx);
            conns.truncate(total_conns - idx);
            total_evicted += idx;
            true
        }
    });
    trace!(target: "http_client::evict", total_evicted, remaining_authorities=conns.len(), "connection eviction complete");
}

impl<T> ConnPool<T> {
    /// Evicts connections from both HTTP and HTTPS pools that have been idle
    /// for longer than the specified timeout.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Duration after which connections are considered idle
    pub async fn evict(&self, timeout: Duration) {
        let Some(cutoff) = Instant::now().checked_sub(timeout) else {
            return;
        };
        join!(
            async {
                let mut conns = self.http.write().await;
                evict_conns(cutoff, &mut conns);
            },
            async {
                let mut conns = self.https.write().await;
                evict_conns(cutoff, &mut conns);
            }
        );
    }

    /// Attempts to get an HTTP connection for the specified authority.
    /// If a cached connection is available, it will be returned as a `Hit`.
    /// Otherwise, a new connection will be established and returned as a `Miss`.
    ///
    /// # Arguments
    ///
    /// * `authority` - The authority (host:port) to connect to
    ///
    /// # Returns
    ///
    /// A cacheable connection or an error if the connection fails
    #[allow(dead_code)]
    pub async fn connect_http(
        &self,
        authority: &str,
    ) -> Result<Cacheable<PooledConn<http1::SendRequest<T>>>, ErrorCode>
    where
        T: http_body::Body + Send + 'static,
        T::Data: Send,
        T::Error: Into<Box<dyn Error + Send + Sync>>,
    {
        trace!(target: "http_client::connect_http", authority, "attempting HTTP connection");
        {
            let http = self.http.read().await;
            if let Some(conns) = http.get(authority) {
                if let Ok(mut conns) = conns.lock() {
                    trace!(target: "http_client::connect_http", authority, cached_connections=conns.len(), "checking cached HTTP connections");
                    while let Some(conn) = conns.pop_front() {
                        trace!(target: "http_client::connect_http", authority, "found cached HTTP connection");
                        if !conn.is_closed() && conn.is_ready() {
                            trace!(target: "http_client::connect_http", authority, "returning HTTP connection cache hit");
                            return Ok(Cacheable::Hit(conn));
                        } else {
                            trace!(target: "http_client::connect_http", authority, is_closed=conn.is_closed(), is_ready=conn.is_ready(), "discarding unusable cached HTTP connection");
                        }
                    }
                }
            }
        }
        trace!(target: "http_client::connect_http", authority, "establishing new TCP connection");
        let stream = connect(authority).await?;
        trace!(target: "http_client::connect_http", authority, "starting HTTP handshake");
        let (sender, conn) = http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|err| {
                warn!(target: "http_client::connect_http", error=?err, authority, "HTTP handshake failed");
                hyper_request_error(err)
            })?;
        let tasks = Arc::clone(&self.tasks);
        let authority_clone = authority.to_string();
        let abort = tasks.lock().await.spawn(async move {
            match conn.await {
                Ok(()) => trace!(target: "http_client::connect_http", authority=authority_clone, "HTTP connection closed successfully"),
                Err(err) => warn!(target: "http_client::connect_http", ?err, authority=authority_clone, "HTTP connection closed with error"),
            }
        });
        trace!(target: "http_client::connect_http", authority, "returning HTTP connection cache miss");
        Ok(Cacheable::Miss(PooledConn::new(sender, abort)))
    }

    #[cfg(any(target_arch = "riscv64", target_arch = "s390x"))]
    pub async fn connect_https(
        &self,
        _tls: &tokio_rustls::TlsConnector,
        _authority: &str,
    ) -> Result<Cacheable<PooledConn<http1::SendRequest<T>>>, ErrorCode>
    where
        T: http_body::Body + Send + 'static,
        T::Data: Send,
        T::Error: Into<Box<dyn Error + Send + Sync>>,
    {
        Err(ErrorCode::UnsupportedArchitecture)
    }

    /// Attempts to get an HTTPS connection for the specified authority.
    /// If a cached connection is available, it will be returned as a `Hit`.
    /// Otherwise, a new connection will be established and returned as a `Miss`.
    ///
    /// # Arguments
    ///
    /// * `tls` - The TLS connector to use for establishing secure connections
    /// * `authority` - The authority (host:port) to connect to
    ///
    /// # Returns
    ///
    /// A cacheable connection or an error if the connection fails
    #[cfg(not(any(target_arch = "riscv64", target_arch = "s390x")))]
    pub async fn connect_https(
        &self,
        tls: &tokio_rustls::TlsConnector,
        authority: &str,
    ) -> Result<Cacheable<PooledConn<http1::SendRequest<T>>>, ErrorCode>
    where
        T: http_body::Body + Send + 'static,
        T::Data: Send,
        T::Error: Into<Box<dyn Error + Send + Sync>>,
    {
        use rustls::pki_types::ServerName;

        trace!(target: "http_client::connect_https", authority, "attempting HTTPS connection");
        {
            let https = self.https.read().await;
            if let Some(conns) = https.get(authority) {
                if let Ok(mut conns) = conns.lock() {
                    trace!(target: "http_client::connect_https", authority, cached_connections=conns.len(), "checking cached HTTPS connections");
                    while let Some(conn) = conns.pop_front() {
                        trace!(target: "http_client::connect_https", authority, "found cached HTTPS connection");
                        if !conn.is_closed() && conn.is_ready() {
                            trace!(target: "http_client::connect_https", authority, "returning HTTPS connection cache hit");
                            return Ok(Cacheable::Hit(conn));
                        } else {
                            trace!(target: "http_client::connect_https", authority, is_closed=conn.is_closed(), is_ready=conn.is_ready(), "discarding unusable cached HTTPS connection");
                        }
                    }
                }
            }
        }
        trace!(target: "http_client::connect_https", authority, "establishing new TCP connection");
        let stream = connect(authority).await?;

        let mut parts = authority.split(":");
        let host = parts.next().unwrap_or(authority);
        trace!(target: "http_client::connect_https", authority, host, "resolving server name for TLS");
        let domain = ServerName::try_from(host)
            .map_err(|err| {
                warn!(target: "http_client::connect_https", ?err, authority, host, "invalid DNS name for TLS");
                dns_error("invalid DNS name".to_string(), 0)
            })?
            .to_owned();
        trace!(target: "http_client::connect_https", authority, host, "starting TLS handshake");
        let stream = tls.connect(domain, stream).await.map_err(|err| {
            warn!(target: "http_client::connect_https", ?err, authority, host, "TLS handshake failed");
            ErrorCode::TlsProtocolError
        })?;
        trace!(target: "http_client::connect_https", authority, "starting HTTP handshake over TLS");
        let (sender, conn) = http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|err| {
                warn!(target: "http_client::connect_https", error=?err, authority, "HTTP handshake failed over TLS");
                hyper_request_error(err)
            })?;
        let tasks = Arc::clone(&self.tasks);
        let authority_clone = authority.to_string();
        let abort = tasks.lock().await.spawn(async move {
            match conn.await {
                Ok(()) => trace!(target: "http_client::connect_https", authority=authority_clone, "HTTPS connection closed successfully"),
                Err(err) => warn!(target: "http_client::connect_https", ?err, authority=authority_clone, "HTTPS connection closed with error"),
            }
        });
        trace!(target: "http_client::connect_https", authority, "returning HTTPS connection cache miss");
        Ok(Cacheable::Miss(PooledConn::new(sender, abort)))
    }
}

/// Represents a cacheable result that can either be a miss or a hit.
/// Used to handle connection caching in the HTTP client provider.
///
/// A `Miss` indicates a newly created connection, while a `Hit` indicates
/// a connection that was reused from the pool.
pub enum Cacheable<T> {
    /// A newly created connection
    Miss(T),
    /// A connection reused from the pool
    Hit(T),
}

impl<T> Deref for Cacheable<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Miss(v) | Self::Hit(v) => v,
        }
    }
}

impl<T> DerefMut for Cacheable<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Miss(v) | Self::Hit(v) => v,
        }
    }
}

impl<T> Cacheable<T> {
    /// Unwraps the inner value, discarding the cache hit/miss information.
    #[allow(dead_code)]
    pub fn unwrap(self) -> T {
        match self {
            Self::Miss(v) => {
                trace!(target: "http_client::cache", "unwrapping cache miss");
                v
            }
            Self::Hit(v) => {
                trace!(target: "http_client::cache", "unwrapping cache hit");
                v
            }
        }
    }
}

/// Translate a DNS error code to a wRPC HTTP error code.
///
/// # Arguments
///
/// * `rcode` - The DNS error code
/// * `info_code` - The DNS info code
///
/// # Returns
///
/// A wRPC HTTP error code representing the DNS error
fn dns_error(rcode: String, info_code: u16) -> ErrorCode {
    ErrorCode::DnsError(DnsErrorPayload {
        rcode: Some(rcode),
        info_code: Some(info_code),
    })
}

/// Establishes a TCP connection to the specified address.
///
/// # Arguments
///
/// * `addr` - The address to connect to
///
/// # Returns
///
/// A TCP stream if successful, or an error if the connection fails
async fn connect(addr: impl ToSocketAddrs) -> Result<TcpStream, ErrorCode> {
    trace!(target: "http_client::connect", "attempting TCP connection");
    match TcpStream::connect(addr).await {
        Ok(stream) => {
            trace!(target: "http_client::connect", "TCP connection established successfully");
            Ok(stream)
        }
        Err(err) if err.kind() == std::io::ErrorKind::AddrNotAvailable => {
            warn!(target: "http_client::connect", error=?err, "address not available");
            Err(dns_error("address not available".to_string(), 0))
        }
        Err(err) => {
            if err
                .to_string()
                .starts_with("failed to lookup address information")
            {
                warn!(target: "http_client::connect", error=?err, "DNS lookup failed");
                Err(dns_error("address not available".to_string(), 0))
            } else {
                warn!(target: "http_client::connect", error=?err, "connection refused");
                Err(ErrorCode::ConnectionRefused)
            }
        }
    }
}

/// Translate a [`hyper::Error`] to a wRPC HTTP error code.
///
/// # Arguments
///
/// * `err` - The hyper error to translate
///
/// # Returns
///
/// A wRPC HTTP error code representing the hyper error
pub fn hyper_request_error(err: hyper::Error) -> ErrorCode {
    // If there's a source, we might be able to extract an error from it.
    if let Some(cause) = err.source() {
        // We can't downcast to E since it's a trait, not a concrete type
        // Just log the error and return a generic HTTP protocol error
        warn!(
            target: "http_client::error",
            error=?err,
            cause=?cause,
            error_type="hyper_with_cause",
            "HTTP request failed with underlying cause"
        );
        return ErrorCode::HttpProtocolError;
    }

    warn!(
        target: "http_client::error",
        error=?err,
        error_type="hyper",
        "HTTP request failed"
    );

    ErrorCode::HttpProtocolError
}

#[cfg(test)]
mod tests {
    use core::net::Ipv4Addr;

    use std::collections::{HashMap, VecDeque};
    use std::time::Instant;

    use anyhow::Context as _;
    use bytes::Bytes;
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpListener;
    use tokio::spawn;
    use tokio::try_join;
    use tracing::info;

    use super::*;
    use wrpc_interface_http::HttpBody;

    const N: usize = 20;

    /// Tests the connection eviction logic by verifying that connections older than the cutoff time are removed
    #[test_log::test(tokio::test(flavor = "multi_thread"))]
    async fn test_conn_evict() -> anyhow::Result<()> {
        let now = Instant::now();

        let mut foo = VecDeque::from([
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now
                    .checked_sub(Duration::from_secs(10))
                    .expect("time subtraction should not overflow"),
            },
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now
                    .checked_sub(Duration::from_secs(1))
                    .expect("time subtraction should not overflow"),
            },
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now,
            },
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now
                    .checked_add(Duration::from_secs(1))
                    .expect("time addition should not overflow"),
            },
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now
                    .checked_add(Duration::from_secs(1))
                    .expect("time addition should not overflow"),
            },
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now
                    .checked_add(Duration::from_secs(3))
                    .expect("time addition should not overflow"),
            },
        ]);
        let qux = VecDeque::from([
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now
                    .checked_add(Duration::from_secs(10))
                    .expect("time addition should not overflow"),
            },
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now
                    .checked_add(Duration::from_secs(12))
                    .expect("time addition should not overflow"),
            },
        ]);
        let mut conns = HashMap::from([
            ("foo".into(), std::sync::Mutex::new(foo.clone())),
            ("bar".into(), std::sync::Mutex::default()),
            (
                "baz".into(),
                std::sync::Mutex::new(VecDeque::from([
                    PooledConn {
                        sender: (),
                        abort: spawn(async {}).abort_handle(),
                        last_seen: now
                            .checked_sub(Duration::from_secs(10))
                            .expect("time subtraction should not overflow"),
                    },
                    PooledConn {
                        sender: (),
                        abort: spawn(async {}).abort_handle(),
                        last_seen: now
                            .checked_sub(Duration::from_secs(1))
                            .expect("time subtraction should not overflow"),
                    },
                ])),
            ),
            ("qux".into(), std::sync::Mutex::new(qux.clone())),
        ]);
        evict_conns(now, &mut conns);
        assert_eq!(
            conns
                .remove("foo")
                .expect("foo should exist")
                .into_inner()
                .expect("mutex should be unlocked"),
            foo.split_off(3)
        );
        assert_eq!(
            conns
                .remove("qux")
                .expect("qux should exist")
                .into_inner()
                .expect("mutex should be unlocked"),
            qux
        );
        assert!(conns.is_empty());
        evict_conns(now, &mut conns);
        assert!(conns.is_empty());
        Ok(())
    }

    /// Tests the connection pool eviction by verifying that idle connections are removed after the timeout period
    #[test_log::test(tokio::test(flavor = "multi_thread"))]
    async fn test_pool_evict() -> anyhow::Result<()> {
        const IDLE_TIMEOUT: Duration = Duration::from_millis(10);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;

        try_join!(
            async {
                for i in 0..N {
                    info!(i, "accepting stream...");
                    let (stream, _) = listener
                        .accept()
                        .await
                        .with_context(|| format!("failed to accept connection {i}"))?;
                    info!(i, "serving connection...");
                    hyper::server::conn::http1::Builder::new()
                        .serve_connection(
                            TokioIo::new(stream),
                            hyper::service::service_fn(move |_| async {
                                anyhow::Ok(http::Response::new(
                                    http_body_util::Empty::<Bytes>::new(),
                                ))
                            }),
                        )
                        .await
                        .with_context(|| format!("failed to serve connection {i}"))?;
                }
                anyhow::Ok(())
            },
            async {
                let pool = ConnPool::<HttpBody>::default();
                let now = Instant::now();

                // Add some connections to the pool
                let mut http_conns = pool.http.write().await;
                let (sender, _) =
                    http1::handshake(TokioIo::new(TcpStream::connect(addr).await?)).await?;

                http_conns.insert(
                    addr.to_string().into(),
                    std::sync::Mutex::new(VecDeque::from([PooledConn {
                        sender,
                        abort: spawn(async {}).abort_handle(),
                        last_seen: now
                            .checked_sub(Duration::from_secs(10))
                            .expect("time subtraction should not overflow"),
                    }])),
                );

                // Evict connections
                pool.evict(IDLE_TIMEOUT).await;

                // Verify connections were evicted
                let http_conns = pool.http.read().await;
                let test_conns = http_conns
                    .get(addr.to_string().into_boxed_str().as_ref())
                    .expect("connection should exist")
                    .lock()
                    .expect("lock should succeed");
                assert_eq!(test_conns.len(), 0);

                Ok(())
            }
        )?;
        Ok(())
    }
}
