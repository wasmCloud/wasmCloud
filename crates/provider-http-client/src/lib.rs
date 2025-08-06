use core::convert::Infallible;
use core::pin::pin;
use core::time::Duration;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context as _;
use bytes::Bytes;
use futures::StreamExt as _;
use http::uri::Scheme;
use http_body::Frame;
use http_body_util::{BodyExt as _, StreamBody};
use tokio::task::JoinSet;
use tokio::time::sleep;
use tokio::{select, spawn};
use tracing::{debug, error, info, trace, warn, Instrument as _};

use wasmcloud_provider_sdk::core::tls;
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, load_host_data, propagate_trace_for_ctx,
    run_provider, Context, Provider,
};
use wrpc_interface_http::{
    bindings::wrpc::http::types::{ErrorCode, RequestOptions},
    split_outgoing_http_body, try_fields_to_header_map, ServeHttp, ServeOutgoingHandlerHttp,
};

// Import shared connection pooling infrastructure from the internal provider
use wasmcloud_core::http_client::{
    hyper_request_error, Cacheable, ConnPool, DEFAULT_CONNECT_TIMEOUT, DEFAULT_FIRST_BYTE_TIMEOUT,
    DEFAULT_IDLE_TIMEOUT, DEFAULT_USER_AGENT, LOAD_NATIVE_CERTS, LOAD_WEBPKI_CERTS, SSL_CERTS_FILE,
};

/// HTTP client capability provider implementation struct
#[derive(Clone)]
pub struct HttpClientProvider {
    /// TLS connector for establishing secure HTTPS connections
    tls: tokio_rustls::TlsConnector,
    /// Connection pools for HTTP and HTTPS connections
    conns: ConnPool<wrpc_interface_http::HttpBody>,
    /// Background tasks for connection management
    #[allow(unused)]
    tasks: Arc<JoinSet<()>>,
}

pub async fn run() -> anyhow::Result<()> {
    info!("Starting HTTP client provider");
    initialize_observability!(
        "http-client-provider",
        std::env::var_os("PROVIDER_HTTP_CLIENT_FLAMEGRAPH_PATH")
    );

    let host_data = load_host_data()?;
    let provider = HttpClientProvider::new(&host_data.config, DEFAULT_IDLE_TIMEOUT).await?;

    debug!("Initializing provider runtime");
    let shutdown = run_provider(provider.clone(), "http-client-provider")
        .await
        .context("failed to run provider")?;

    let connection = get_connection();
    let wrpc = connection
        .get_wrpc_client(connection.provider_key())
        .await?;

    debug!("Setting up wrpc interface");
    let [(_, _, mut invocations)] =
        wrpc_interface_http::bindings::exports::wrpc::http::outgoing_handler::serve_interface(
            &wrpc,
            ServeHttp(provider),
        )
        .await
        .context("failed to serve exports")?;

    info!("HTTP client provider ready to handle requests");
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
                info!("Received shutdown signal");
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
        debug!("Creating new HTTP client provider");

        // Initialize TLS configuration
        let tls = if config.is_empty() {
            debug!("Using default TLS connector");
            tls::DEFAULT_RUSTLS_CONNECTOR.clone()
        } else {
            debug!("Configuring custom TLS connector");
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

        // Initialize connection pool and eviction task
        let conns = ConnPool::default();
        let mut tasks = JoinSet::new();

        debug!(
            "Starting connection eviction task with timeout: {:?}",
            idle_timeout
        );
        tasks.spawn({
            let conns = conns.clone();
            async move {
                loop {
                    sleep(idle_timeout).await;
                    trace!("Evicting idle connections");
                    conns.evict(idle_timeout).await;
                }
            }
        });

        debug!("HTTP client provider initialization complete");
        Ok(Self {
            tls,
            conns,
            tasks: Arc::new(tasks),
        })
    }
}

impl ServeOutgoingHandlerHttp<Option<Context>> for HttpClientProvider {
    #[tracing::instrument(level = "debug", skip_all)]
    async fn handle(
        &self,
        cx: Option<Context>,
        mut request: http::Request<wrpc_interface_http::HttpBody>,
        options: Option<RequestOptions>,
    ) -> anyhow::Result<
        Result<
            http::Response<impl http_body::Body<Data = Bytes, Error = Infallible> + Send + 'static>,
            ErrorCode,
        >,
    > {
        info!(
            method = %request.method(),
            uri = %request.uri(),
            "Handling outgoing HTTP request"
        );

        propagate_trace_for_ctx!(cx);
        wasmcloud_provider_sdk::wasmcloud_tracing::http::HeaderInjector(request.headers_mut())
            .inject_context();

        debug!(headers = ?request.headers(), "Request headers");

        let connect_timeout = options
            .and_then(|opts| opts.connect_timeout.map(Duration::from_nanos))
            .unwrap_or(DEFAULT_CONNECT_TIMEOUT);

        let first_byte_timeout = options
            .and_then(|opts| opts.first_byte_timeout.map(Duration::from_nanos))
            .unwrap_or(DEFAULT_FIRST_BYTE_TIMEOUT);

        debug!(
            ?connect_timeout,
            ?first_byte_timeout,
            "Request timeouts configured"
        );

        Ok(async {
            let authority = request
                .uri()
                .authority()
                .ok_or(ErrorCode::HttpRequestUriInvalid)?;

            debug!(%authority, "Request authority extracted");

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

            debug!(%authority, use_tls, "Using authority with TLS setting");

            // Remove scheme and authority from request URI
            *request.uri_mut() = http::Uri::builder()
                .path_and_query(
                    request
                        .uri()
                        .path_and_query()
                        .map(|p| p.as_str())
                        .unwrap_or("/"),
                )
                .build()
                .map_err(|err| ErrorCode::InternalError(Some(err.to_string())))?;

            // Ensure User-Agent header is set
            request
                .headers_mut()
                .entry(http::header::USER_AGENT)
                .or_insert(http::header::HeaderValue::from_static(DEFAULT_USER_AGENT));

            debug!(path = %request.uri().path(), "Request URI prepared for sending");

            loop {
                let mut sender = if use_tls {
                    debug!(%authority, "Establishing HTTPS connection");
                    tokio::time::timeout(
                        connect_timeout,
                        self.conns.connect_https(&self.tls, &authority),
                    )
                    .await
                } else {
                    debug!(%authority, "Establishing HTTP connection");
                    tokio::time::timeout(connect_timeout, self.conns.connect_http(&authority)).await
                }
                .map_err(|_| ErrorCode::ConnectionTimeout)??;

                debug!(
                    uri = ?request.uri(),
                    method = %request.method(),
                    connection_type = if use_tls { "HTTPS" } else { "HTTP" },
                    is_cached = matches!(sender, Cacheable::Hit(..)),
                    "Sending HTTP request"
                );

                match tokio::time::timeout(first_byte_timeout, sender.try_send_request(request))
                    .instrument(tracing::debug_span!("http_request"))
                    .await
                    .map_err(|_| ErrorCode::ConnectionReadTimeout)?
                {
                    Err(mut err) => {
                        let req = err.take_message();
                        let err = err.into_error();
                        if let Some(req) = req {
                            if err.is_closed() && matches!(sender, Cacheable::Hit(..)) {
                                debug!(%authority, "Cached connection closed, retrying with a different connection");
                                request = req;
                                continue;
                            }
                        }
                        warn!(?err, %authority, "HTTP request error");
                        return Err(hyper_request_error(err));
                    }
                    Ok(res) => {
                        debug!(%authority, status = %res.status(), "HTTP response received");

                        let authority = authority.into_boxed_str();
                        let mut sender = sender.unwrap();
                        if use_tls {
                            let mut https = self.conns.https.write().await;
                            sender.last_seen = std::time::Instant::now();
                            if let Ok(conns) = https.entry(authority).or_default().get_mut() {
                                debug!("Caching HTTPS connection for future use");
                                conns.push_front(sender);
                            }
                        } else {
                            let mut http = self.conns.http.write().await;
                            sender.last_seen = std::time::Instant::now();
                            if let Ok(conns) = http.entry(authority).or_default().get_mut() {
                                debug!("Caching HTTP connection for future use");
                                conns.push_front(sender);
                            }
                        }

                        return Ok(res.map(|body| {
                            let (data, trailers, mut errs) = split_outgoing_http_body(body);
                            spawn(
                                async move {
                                    while let Some(err) = errs.next().await {
                                        error!(?err, "Body error encountered");
                                    }
                                    trace!("Body processing finished");
                                }
                                .in_current_span(),
                            );
                            StreamBody::new(data.map(Frame::data).map(Ok)).with_trailers(async {
                                trace!("Awaiting trailers");
                                if let Some(trailers) = trailers.await {
                                    trace!("Trailers received");
                                    match try_fields_to_header_map(trailers) {
                                        Ok(headers) => Some(Ok(headers)),
                                        Err(err) => {
                                            error!(?err, "Failed to parse trailers");
                                            None
                                        }
                                    }
                                } else {
                                    trace!("No trailers received");
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

impl Provider for HttpClientProvider {}

#[cfg(test)]
mod tests {
    use core::net::{Ipv4Addr, SocketAddr};
    use core::sync::atomic::{AtomicUsize, Ordering};

    use std::collections::HashMap;

    use anyhow::{ensure, Context as _};
    use bytes::Bytes;
    use hyper_util::rt::TokioIo;
    use tokio::net::TcpListener;
    use tokio::spawn;
    use tokio::try_join;
    use tracing::info;

    use super::*;
    use wrpc_interface_http::HttpBody;

    const N: usize = 20;

    fn new_request(addr: SocketAddr) -> http::Request<HttpBody> {
        http::Request::builder()
            .method(http::Method::POST)
            .uri(format!("http://{addr}"))
            .body(HttpBody {
                body: Box::pin(futures::stream::empty()),
                trailers: Box::pin(async { None }),
            })
            .expect("failed to construct HTTP POST request")
    }

    /// Tests connection reuse by verifying that multiple requests use the same connection
    #[test_log::test(tokio::test(flavor = "multi_thread"))]
    #[test_log(default_log_filter = "trace")]
    async fn test_reuse_conn() -> anyhow::Result<()> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        let requests = AtomicUsize::default();
        try_join!(
            async {
                let mut conns: usize = 0;
                while requests.load(Ordering::Relaxed) != N {
                    info!("accepting stream...");
                    let (stream, _) = listener
                        .accept()
                        .await
                        .context("failed to accept connection")?;
                    info!(i = conns, "serving connection...");
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
                    info!(i = conns, "done serving connection");
                    conns = conns.saturating_add(1);
                }
                let reqs = requests.load(Ordering::Relaxed);
                info!(connections = conns, requests = reqs, "server finished");
                ensure!(conns < reqs, "connections: {conns}, requests: {reqs}");
                anyhow::Ok(())
            },
            async {
                let provider =
                    HttpClientProvider::new(&HashMap::default(), DEFAULT_IDLE_TIMEOUT).await?;
                for i in 0..N {
                    info!(i, "sending request...");
                    let res =
                        provider
                            .handle(
                                None,
                                new_request(addr),
                                Some(RequestOptions {
                                    connect_timeout: Some(Duration::from_secs(10).as_nanos() as _),
                                    first_byte_timeout: Some(
                                        Duration::from_secs(10).as_nanos() as _
                                    ),
                                    between_bytes_timeout: Some(
                                        Duration::from_secs(10).as_nanos() as _
                                    ),
                                }),
                            )
                            .await
                            .with_context(|| format!("failed to invoke `handle` for request {i}"))?
                            .with_context(|| format!("failed to handle request {i}"))?;
                    requests.store(i.saturating_add(1), Ordering::Relaxed);
                    info!(i, "reading response body...");
                    let body = res.collect().await?;
                    assert_eq!(body.to_bytes(), Bytes::default());
                }
                Ok(())
            }
        )?;
        Ok(())
    }

    /// Tests handling of concurrent connections by verifying multiple simultaneous requests
    #[test_log::test(tokio::test(flavor = "multi_thread"))]
    async fn test_concurrent_conn() -> anyhow::Result<()> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        let provider = HttpClientProvider::new(&HashMap::default(), DEFAULT_IDLE_TIMEOUT).await?;
        let mut clt = JoinSet::new();
        for i in 0..N {
            clt.spawn({
                let provider = provider.clone();
                async move {
                    info!(i, "sending request...");
                    let res = provider
                        .handle(None, new_request(addr), None)
                        .await
                        .with_context(|| format!("failed to invoke `handle` for request {i}"))?
                        .with_context(|| format!("failed to handle request {i}"))?;
                    info!(i, "reading response body...");
                    let body = res.collect().await?;
                    assert_eq!(body.to_bytes(), Bytes::default());
                    anyhow::Ok(())
                }
            });
        }
        let mut streams = Vec::with_capacity(N);
        for i in 0..N {
            info!(i, "accepting stream...");
            let (stream, _) = listener
                .accept()
                .await
                .with_context(|| format!("failed to accept connection {i}"))?;
            streams.push(stream);
        }

        let mut srv = JoinSet::new();
        for stream in streams {
            srv.spawn(async {
                info!("serving connection...");
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
        Ok(())
    }

    /// Tests error handling by verifying proper handling of HTTP error responses
    #[test_log::test(tokio::test(flavor = "multi_thread"))]
    async fn test_http_error_handling() -> anyhow::Result<()> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        let provider = HttpClientProvider::new(&HashMap::default(), DEFAULT_IDLE_TIMEOUT).await?;
        let request = new_request(addr);

        // Spawn server that returns error responses
        spawn(async move {
            let (stream, _) = listener.accept().await?;
            hyper::server::conn::http1::Builder::new()
                .serve_connection(
                    TokioIo::new(stream),
                    hyper::service::service_fn(move |_| async {
                        anyhow::Ok(
                            http::Response::builder()
                                .status(http::StatusCode::INTERNAL_SERVER_ERROR)
                                .body(http_body_util::Empty::<Bytes>::new())?,
                        )
                    }),
                )
                .await?;
            Ok::<_, anyhow::Error>(())
        });

        // Send request and verify error handling
        let result = provider.handle(None, request, None).await?;
        assert!(result.is_ok());
        let response = result?;
        assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);

        Ok(())
    }
}
