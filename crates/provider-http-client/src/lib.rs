use core::convert::Infallible;
use core::error::Error;
use core::ops::{Deref, DerefMut};
use core::pin::pin;
use core::time::Duration;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use anyhow::Context as _;
use bytes::Bytes;
use futures::StreamExt as _;
use http::uri::Scheme;
use http_body::Frame;
use http_body_util::{BodyExt as _, StreamBody};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::net::{TcpStream, ToSocketAddrs};
use tokio::sync::Mutex;
use tokio::task::{AbortHandle, JoinSet};
use tokio::time::sleep;
use tokio::{join, sync::RwLock};
use tokio::{select, spawn};
use tracing::{debug, error, instrument, trace, warn, Instrument as _};

use wasmcloud_provider_sdk::core::tls;
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, load_host_data, propagate_trace_for_ctx,
    run_provider, Context, Provider,
};
use wrpc_interface_http::bindings::wrpc::http::types;
use wrpc_interface_http::{
    split_outgoing_http_body, try_fields_to_header_map, ServeHttp, ServeOutgoingHandlerHttp,
};

// adapted from https://github.com/hyperium/hyper-util/blob/46826ea75836852fac53ff075a12cba7e290946e/src/client/legacy/client.rs#L1004
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

// Instant used as the "zero" `last_seen` value.
static ZERO_INSTANT: LazyLock<Instant> = LazyLock::new(Instant::now);

/// HTTP client capability provider implementation struct
#[derive(Clone)]
pub struct HttpClientProvider {
    tls: tokio_rustls::TlsConnector,
    conns: ConnPool<wrpc_interface_http::HttpBody>,
    #[allow(unused)]
    tasks: Arc<JoinSet<()>>,
}

#[derive(Clone, Debug)]
struct PooledConn<T> {
    sender: T,
    abort: AbortHandle,
    last_seen: Instant,
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
    fn new(sender: T, abort: AbortHandle) -> Self {
        Self {
            sender,
            abort,
            last_seen: *ZERO_INSTANT,
        }
    }
}

type ConnPoolTable<T> =
    RwLock<HashMap<Box<str>, std::sync::Mutex<VecDeque<PooledConn<http1::SendRequest<T>>>>>>;

#[derive(Debug)]
struct ConnPool<T> {
    http: Arc<ConnPoolTable<T>>,
    https: Arc<ConnPoolTable<T>>,
    tasks: Arc<Mutex<JoinSet<()>>>,
}

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

fn evict_conns<T>(
    cutoff: Instant,
    conns: &mut HashMap<Box<str>, std::sync::Mutex<VecDeque<PooledConn<T>>>>,
) {
    conns.retain(|_, conns| {
        let Ok(conns) = conns.get_mut() else {
            return true;
        };
        let idx = conns.partition_point(|&PooledConn { last_seen, .. }| last_seen <= cutoff);
        if idx == conns.len() {
            false
        } else if idx == 0 {
            true
        } else {
            conns.rotate_left(idx);
            conns.truncate(idx);
            true
        }
    });
}

impl<T> ConnPool<T> {
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
}

async fn connect(addr: impl ToSocketAddrs) -> Result<TcpStream, types::ErrorCode> {
    match TcpStream::connect(addr).await {
        Ok(stream) => Ok(stream),
        Err(err) if err.kind() == std::io::ErrorKind::AddrNotAvailable => {
            Err(dns_error("address not available".to_string(), 0))
        }
        Err(err) => {
            if err
                .to_string()
                .starts_with("failed to lookup address information")
            {
                Err(dns_error("address not available".to_string(), 0))
            } else {
                Err(types::ErrorCode::ConnectionRefused)
            }
        }
    }
}

enum Cacheable<T> {
    Miss(T),
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
    pub fn unwrap(self) -> T {
        match self {
            Self::Miss(v) | Self::Hit(v) => v,
        }
    }
}

impl<T> ConnPool<T> {
    pub async fn connect_http(
        &self,
        authority: &str,
    ) -> Result<Cacheable<PooledConn<http1::SendRequest<T>>>, types::ErrorCode>
    where
        T: http_body::Body + Send + 'static,
        T::Data: Send,
        T::Error: Into<Box<dyn Error + Send + Sync>>,
    {
        {
            let http = self.http.read().await;
            if let Some(conns) = http.get(authority) {
                if let Ok(mut conns) = conns.lock() {
                    while let Some(conn) = conns.pop_front() {
                        if !conn.is_closed() && conn.is_ready() {
                            return Ok(Cacheable::Hit(conn));
                        }
                    }
                }
            }
        }
        let stream = connect(authority).await?;
        let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(hyper_request_error)?;
        let tasks = Arc::clone(&self.tasks);
        let abort = tasks.lock().await.spawn(async move {
            match conn.await {
                Ok(()) => trace!("HTTP connection closed successfully"),
                Err(err) => warn!(?err, "HTTP connection closed with error"),
            }
        });
        Ok(Cacheable::Miss(PooledConn::new(sender, abort)))
    }

    #[cfg(any(target_arch = "riscv64", target_arch = "s390x"))]
    pub async fn connect_https(
        &self,
        _authority: &str,
    ) -> Result<Cacheable<PooledConn<http1::SendRequest<T>>>, types::ErrorCode> {
        Err(types::ErrorCode::InternalError(Some(
            "unsupported architecture for SSL".to_string(),
        )));
    }

    #[cfg(not(any(target_arch = "riscv64", target_arch = "s390x")))]
    pub async fn connect_https(
        &self,
        tls: &tokio_rustls::TlsConnector,
        authority: &str,
    ) -> Result<Cacheable<PooledConn<http1::SendRequest<T>>>, types::ErrorCode>
    where
        T: http_body::Body + Send + 'static,
        T::Data: Send,
        T::Error: Into<Box<dyn Error + Send + Sync>>,
    {
        use rustls::pki_types::ServerName;

        {
            let https = self.https.read().await;
            if let Some(conns) = https.get(authority) {
                if let Ok(mut conns) = conns.lock() {
                    while let Some(conn) = conns.pop_front() {
                        if !conn.is_closed() && conn.is_ready() {
                            return Ok(Cacheable::Hit(conn));
                        }
                    }
                }
            }
        }
        let stream = connect(authority).await?;

        let mut parts = authority.split(":");
        let host = parts.next().unwrap_or(authority);
        let domain = ServerName::try_from(host)
            .map_err(|err| {
                warn!(?err, "DNS lookup failed");
                dns_error("invalid DNS name".to_string(), 0)
            })?
            .to_owned();
        let stream = tls.connect(domain, stream).await.map_err(|err| {
            warn!(?err, "TLS protocol error");
            types::ErrorCode::TlsProtocolError
        })?;

        let (sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(hyper_request_error)?;
        let tasks = Arc::clone(&self.tasks);
        let abort = tasks.lock().await.spawn(async move {
            match conn.await {
                Ok(()) => trace!("HTTPS connection closed successfully"),
                Err(err) => warn!(?err, "HTTPS connection closed with error"),
            }
        });
        Ok(Cacheable::Miss(PooledConn::new(sender, abort)))
    }
}

const DEFAULT_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
// Configuration
const LOAD_NATIVE_CERTS: &str = "load_native_certs";
const LOAD_WEBPKI_CERTS: &str = "load_webpki_certs";
const SSL_CERTS_FILE: &str = "ssl_certs_file";

pub async fn run() -> anyhow::Result<()> {
    initialize_observability!(
        "http-client-provider",
        std::env::var_os("PROVIDER_HTTP_CLIENT_FLAMEGRAPH_PATH")
    );
    let host_data = load_host_data()?;
    let provider = HttpClientProvider::new(&host_data.config, DEFAULT_IDLE_TIMEOUT).await?;
    let shutdown = run_provider(provider.clone(), "http-client-provider")
        .await
        .context("failed to run provider")?;
    let connection = get_connection();
    let wrpc = connection
        .get_wrpc_client(connection.provider_key())
        .await?;
    let [(_, _, mut invocations)] =
        wrpc_interface_http::bindings::exports::wrpc::http::outgoing_handler::serve_interface(
            &wrpc,
            ServeHttp(provider),
        )
        .await
        .context("failed to serve exports")?;
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();
    loop {
        select! {
            Some(res) = invocations.next() => {
                match res {
                    Ok(fut) => {
                        tasks.spawn(async move {
                            if let Err(err) = fut.await {
                                warn!(?err, "failed to serve invocation");
                            }
                        });
                    },
                    Err(err) => {
                        warn!(?err, "failed to accept invocation");
                    }
                }
            },
            () = &mut shutdown => {
                return Ok(())
            }
        }
    }
}

impl HttpClientProvider {
    pub async fn new(
        config: &HashMap<String, String>,
        idle_timeout: Duration,
    ) -> anyhow::Result<Self> {
        // Short circuit to the default connector if no configuration is provided
        let tls = if config.is_empty() {
            tls::DEFAULT_RUSTLS_CONNECTOR.clone()
        } else {
            let mut ca = rustls::RootCertStore::empty();

            // Load native certificates
            if config
                .get(LOAD_NATIVE_CERTS)
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(true)
            {
                let (added, ignored) =
                    ca.add_parsable_certificates(tls::NATIVE_ROOTS.iter().cloned());
                debug!(added, ignored, "loaded native root certificate store");
            }

            // Load Mozilla trusted root certificates
            if config
                .get(LOAD_WEBPKI_CERTS)
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(true)
            {
                ca.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                debug!("loaded webpki root certificate store");
            }

            // Load root certificates from a file
            if let Some(file_path) = config.get(SSL_CERTS_FILE) {
                let f = std::fs::File::open(file_path)?;
                let mut reader = std::io::BufReader::new(f);
                let certs = rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?;
                let (added, ignored) = ca.add_parsable_certificates(certs);
                debug!(
                    added,
                    ignored, "added additional root certificates from file"
                );
            }
            tokio_rustls::TlsConnector::from(Arc::new(
                rustls::ClientConfig::builder()
                    .with_root_certificates(ca)
                    .with_no_client_auth(),
            ))
        };
        let conns = ConnPool::default();
        let mut tasks = JoinSet::new();
        tasks.spawn({
            let conns = conns.clone();
            async move {
                loop {
                    sleep(idle_timeout).await;
                    conns.evict(idle_timeout).await;
                }
            }
        });
        Ok(Self {
            tls,
            conns,
            tasks: Arc::new(tasks),
        })
    }
}

fn dns_error(rcode: String, info_code: u16) -> types::ErrorCode {
    types::ErrorCode::DnsError(
        wrpc_interface_http::bindings::wasi::http::types::DnsErrorPayload {
            rcode: Some(rcode),
            info_code: Some(info_code),
        },
    )
}

/// Translate a [`hyper::Error`] to a wasi-http `ErrorCode` in the context of a request.
fn hyper_request_error(err: hyper::Error) -> types::ErrorCode {
    // If there's a source, we might be able to extract a wasi-http error from it.
    if let Some(cause) = err.source() {
        if let Some(err) = cause.downcast_ref::<types::ErrorCode>() {
            return err.clone();
        }
    }

    warn!(?err, "hyper request error");

    types::ErrorCode::HttpProtocolError
}

impl ServeOutgoingHandlerHttp<Option<Context>> for HttpClientProvider {
    #[instrument(level = "debug", skip_all)]
    async fn handle(
        &self,
        cx: Option<Context>,
        mut request: http::Request<wrpc_interface_http::HttpBody>,
        options: Option<types::RequestOptions>,
    ) -> anyhow::Result<
        Result<
            http::Response<impl http_body::Body<Data = Bytes, Error = Infallible> + Send + 'static>,
            types::ErrorCode,
        >,
    > {
        propagate_trace_for_ctx!(cx);
        wasmcloud_provider_sdk::wasmcloud_tracing::http::HeaderInjector(request.headers_mut())
            .inject_context();

        // Adapted from:
        // https://github.com/bytecodealliance/wasmtime/blob/d943d57e78950da21dd430e0847f3b8fd0ade073/crates/wasi-http/src/types.rs#L333-L475

        let connect_timeout = options
            .and_then(
                |types::RequestOptions {
                     connect_timeout, ..
                 }| connect_timeout.map(Duration::from_nanos),
            )
            .unwrap_or(Duration::from_secs(600));

        let first_byte_timeout = options
            .and_then(
                |types::RequestOptions {
                     first_byte_timeout, ..
                 }| first_byte_timeout.map(Duration::from_nanos),
            )
            .unwrap_or(Duration::from_secs(600));

        Ok(async {
            let authority = request
                .uri()
                .authority()
                .ok_or(types::ErrorCode::HttpRequestUriInvalid)?;

            let use_tls = match request.uri().scheme() {
                None => true,
                Some(scheme) if *scheme == Scheme::HTTPS => true,
                Some(..) => false,
            };
            let authority = if authority.port().is_some() {
                authority.to_string()
            } else {
                let port = if use_tls { 443 } else { 80 };
                format!("{authority}:{port}")
            };

            // at this point, the request contains the scheme and the authority, but
            // the http packet should only include those if addressing a proxy, so
            // remove them here, since SendRequest::send_request does not do it for us
            *request.uri_mut() = http::Uri::builder()
                .path_and_query(
                    request
                        .uri()
                        .path_and_query()
                        .map(|p| p.as_str())
                        .unwrap_or("/"),
                )
                .build()
                .map_err(|err| types::ErrorCode::InternalError(Some(err.to_string())))?;
            // Ensure we have a User-Agent header set.
            request
                .headers_mut()
                .entry(http::header::USER_AGENT)
                .or_insert(http::header::HeaderValue::from_static(DEFAULT_USER_AGENT));

            loop {
                let mut sender = if use_tls {
                    tokio::time::timeout(
                        connect_timeout,
                        self.conns.connect_https(&self.tls, &authority),
                    )
                    .await
                } else {
                    tokio::time::timeout(connect_timeout, self.conns.connect_http(&authority)).await
                }
                .map_err(|_| types::ErrorCode::ConnectionTimeout)??;

                debug!(uri = ?request.uri(), "sending HTTP request");
                match tokio::time::timeout(first_byte_timeout, sender.try_send_request(request))
                    .instrument(tracing::debug_span!("http_request"))
                    .await
                    .map_err(|_| types::ErrorCode::ConnectionReadTimeout)?
                {
                    Err(mut err) => {
                        let req = err.take_message();
                        let err = err.into_error();
                        if let Some(req) = req {
                            if err.is_closed() && matches!(sender, Cacheable::Hit(..)) {
                                // retry a cached connection
                                request = req;
                                continue;
                            }
                        }
                        return Err(hyper_request_error(err));
                    }
                    Ok(res) => {
                        trace!("HTTP response received");
                        let authority = authority.into_boxed_str();
                        let mut sender = sender.unwrap();
                        if use_tls {
                            let mut https = self.conns.https.write().await;
                            sender.last_seen = Instant::now();
                            if let Ok(conns) = https.entry(authority).or_default().get_mut() {
                                conns.push_front(sender);
                            }
                        } else {
                            let mut http = self.conns.http.write().await;
                            sender.last_seen = Instant::now();
                            if let Ok(conns) = http.entry(authority).or_default().get_mut() {
                                conns.push_front(sender);
                            }
                        }
                        return Ok(res.map(|body| {
                            let (data, trailers, mut errs) = split_outgoing_http_body(body);
                            spawn(
                                async move {
                                    while let Some(err) = errs.next().await {
                                        error!(?err, "body error encountered");
                                    }
                                    trace!("body processing finished");
                                }
                                .in_current_span(),
                            );
                            StreamBody::new(data.map(Frame::data).map(Ok)).with_trailers(async {
                                trace!("awaiting trailers");
                                if let Some(trailers) = trailers.await {
                                    trace!("trailers received");
                                    match try_fields_to_header_map(trailers) {
                                        Ok(headers) => Some(Ok(headers)),
                                        Err(err) => {
                                            error!(?err, "failed to parse trailers");
                                            None
                                        }
                                    }
                                } else {
                                    trace!("no trailers received");
                                    None
                                }
                            })
                        }));
                    }
                }
            }
        }
        .await)
    }
}

/// Handle provider control commands
impl Provider for HttpClientProvider {}

#[cfg(test)]
mod tests {
    use core::net::Ipv4Addr;

    use std::collections::HashMap;

    use http::Request;
    use tokio::net::TcpListener;
    use tokio::try_join;
    use wrpc_interface_http::{HttpBody, ServeOutgoingHandlerHttp};

    use super::*;

    #[test_log::test(tokio::test(flavor = "multi_thread"))]
    async fn test_conn_evict() -> anyhow::Result<()> {
        let now = Instant::now();

        let mut foo = VecDeque::from([
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now.checked_sub(Duration::from_secs(10)).unwrap(),
            },
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now.checked_sub(Duration::from_secs(1)).unwrap(),
            },
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now,
            },
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now.checked_add(Duration::from_secs(1)).unwrap(),
            },
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now.checked_add(Duration::from_secs(1)).unwrap(),
            },
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now.checked_add(Duration::from_secs(3)).unwrap(),
            },
        ]);
        let qux = VecDeque::from([
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now.checked_add(Duration::from_secs(10)).unwrap(),
            },
            PooledConn {
                sender: (),
                abort: spawn(async {}).abort_handle(),
                last_seen: now.checked_add(Duration::from_secs(12)).unwrap(),
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
                        last_seen: now.checked_sub(Duration::from_secs(10)).unwrap(),
                    },
                    PooledConn {
                        sender: (),
                        abort: spawn(async {}).abort_handle(),
                        last_seen: now.checked_sub(Duration::from_secs(1)).unwrap(),
                    },
                ])),
            ),
            ("qux".into(), std::sync::Mutex::new(qux.clone())),
        ]);
        evict_conns(now, &mut conns);
        assert_eq!(
            conns.remove("foo").unwrap().into_inner().unwrap(),
            foo.split_off(3)
        );
        assert_eq!(conns.remove("qux").unwrap().into_inner().unwrap(), qux);
        assert!(conns.is_empty());
        evict_conns(now, &mut conns);
        assert!(conns.is_empty());
        Ok(())
    }

    #[test_log::test(tokio::test(flavor = "multi_thread"))]
    async fn test_single_conn() -> anyhow::Result<()> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        try_join!(
            async {
                let (stream, _) = listener
                    .accept()
                    .await
                    .context("failed to accept connection")?;
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(
                        TokioIo::new(stream),
                        hyper::service::service_fn(move |_| async {
                            anyhow::Ok(http::Response::new(http_body_util::Empty::<Bytes>::new()))
                        }),
                    )
                    .await
                    .context("failed to serve connection")
            },
            async {
                let link =
                    HttpClientProvider::new(&HashMap::default(), DEFAULT_IDLE_TIMEOUT).await?;
                for _ in 0..100 {
                    let request = Request::builder()
                        .method(http::method::Method::POST)
                        .uri(format!("http://{addr}"))
                        .body(HttpBody {
                            body: Box::pin(futures::stream::empty()),
                            trailers: Box::pin(async { None }),
                        })?;
                    let res = link.handle(None, request, None).await??;
                    let body = res.collect().await.context("failed to receive body")?;
                    assert_eq!(body.to_bytes(), Bytes::default());
                }
                drop(link); // drop link to close all pooled connections
                Ok(())
            }
        )?;
        Ok(())
    }

    #[test_log::test(tokio::test(flavor = "multi_thread"))]
    async fn test_concurrent_conn() -> anyhow::Result<()> {
        const N: usize = 10;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        let link = HttpClientProvider::new(&HashMap::default(), DEFAULT_IDLE_TIMEOUT).await?;
        let mut clt = JoinSet::new();
        for _ in 0..N {
            clt.spawn({
                let link = link.clone();
                async move {
                    let request = Request::builder()
                        .method(http::method::Method::POST)
                        .uri(format!("http://{addr}"))
                        .body(HttpBody {
                            body: Box::pin(futures::stream::empty()),
                            trailers: Box::pin(async { None }),
                        })?;
                    let res = link.handle(None, request, None).await??;
                    let body = res.collect().await.context("failed to receive body")?;
                    assert_eq!(body.to_bytes(), Bytes::default());
                    anyhow::Ok(())
                }
            });
        }
        let mut streams = Vec::with_capacity(N);
        for i in 0..N {
            let (stream, _) = listener
                .accept()
                .await
                .with_context(|| format!("failed to accept connection {i}"))?;
            streams.push(stream);
        }

        let mut srv = JoinSet::new();
        for stream in streams {
            srv.spawn(async {
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(
                        TokioIo::new(stream),
                        hyper::service::service_fn(move |_| async {
                            anyhow::Ok(http::Response::new(http_body_util::Empty::<Bytes>::new()))
                        }),
                    )
                    .await
                    .context("failed to serve connection")
            });
        }
        while let Some(res) = clt.join_next().await {
            res??;
        }
        for _ in 0..N {
            // all of these requests should be able to reuse N pooled connections
            clt.spawn({
                let link = link.clone();
                async move {
                    let request = Request::builder()
                        .method(http::method::Method::POST)
                        .uri(format!("http://{addr}"))
                        .body(HttpBody {
                            body: Box::pin(futures::stream::empty()),
                            trailers: Box::pin(async { None }),
                        })?;
                    let res = link.handle(None, request, None).await??;
                    let body = res.collect().await.context("failed to receive body")?;
                    assert_eq!(body.to_bytes(), Bytes::default());
                    anyhow::Ok(())
                }
            });
        }
        while let Some(res) = clt.join_next().await {
            res??;
        }
        drop(link); // drop link to close all pooled connections
        while let Some(res) = srv.join_next().await {
            res??;
        }
        Ok(())
    }

    #[test_log::test(tokio::test(flavor = "multi_thread"))]
    async fn test_pool_evict() -> anyhow::Result<()> {
        const N: usize = 10;
        const IDLE_TIMEOUT: Duration = Duration::from_millis(10);

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        try_join!(
            async {
                for i in 0..N {
                    let (stream, _) = listener
                        .accept()
                        .await
                        .with_context(|| format!("failed to accept connection {i}"))?;
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
                        .context("failed to serve connection")?;
                }
                anyhow::Ok(())
            },
            async {
                let link = HttpClientProvider::new(&HashMap::default(), IDLE_TIMEOUT).await?;
                for _ in 0..N {
                    let request = Request::builder()
                        .method(http::method::Method::POST)
                        .uri(format!("http://{addr}"))
                        .body(HttpBody {
                            body: Box::pin(futures::stream::empty()),
                            trailers: Box::pin(async { None }),
                        })?;
                    let res = link.handle(None, request, None).await??;
                    let body = res.collect().await.context("failed to receive body")?;
                    assert_eq!(body.to_bytes(), Bytes::default());
                    // Pooled connection should be evicted after 2*IDLE_TIMEOUT
                    sleep(IDLE_TIMEOUT.saturating_mul(2)).await;
                }
                Ok(())
            }
        )?;
        Ok(())
    }
}
