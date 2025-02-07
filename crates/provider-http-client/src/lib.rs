use core::convert::Infallible;
use core::pin::pin;
use core::time::Duration;

use std::collections::HashMap;
use std::error::Error as _;
use std::sync::Arc;

use anyhow::Context as _;
use bytes::Bytes;
use futures::StreamExt as _;
use http::uri::Scheme;
use http_body::Frame;
use http_body_util::{BodyExt as _, StreamBody};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio::{select, spawn};
use tracing::{debug, error, instrument, trace, warn, Instrument};

use wasmcloud_provider_sdk::core::tls;
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, load_host_data, propagate_trace_for_ctx,
    run_provider, Context, Provider,
};
use wrpc_interface_http::bindings::wrpc::http::types;
use wrpc_interface_http::{
    split_outgoing_http_body, try_fields_to_header_map, ServeHttp, ServeOutgoingHandlerHttp,
};

/// HTTP client capability provider implementation struct
#[derive(Clone)]
pub struct HttpClientProvider {
    tls: tokio_rustls::TlsConnector,
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
    let provider = HttpClientProvider::new(&host_data.config).await?;
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
    pub async fn new(config: &HashMap<String, String>) -> anyhow::Result<Self> {
        // Short circuit to the default connector if no configuration is provided
        if config.is_empty() {
            return Ok(Self {
                tls: tls::DEFAULT_RUSTLS_CONNECTOR.clone(),
            });
        }

        let mut ca = rustls::RootCertStore::empty();

        // Load native certificates
        if config
            .get(LOAD_NATIVE_CERTS)
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true)
        {
            let (added, ignored) = ca.add_parsable_certificates(tls::NATIVE_ROOTS.iter().cloned());
            tracing::debug!(added, ignored, "loaded native root certificate store");
        }

        // Load Mozilla trusted root certificates
        if config
            .get(LOAD_WEBPKI_CERTS)
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true)
        {
            ca.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            tracing::debug!("loaded webpki root certificate store");
        }

        // Load root certificates from a file
        if let Some(file_path) = config.get(SSL_CERTS_FILE) {
            let f = std::fs::File::open(file_path)?;
            let mut reader = std::io::BufReader::new(f);
            let certs = rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?;
            let (added, ignored) = ca.add_parsable_certificates(certs);
            tracing::debug!(
                added,
                ignored,
                "added additional root certificates from file"
            );
        }

        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(ca)
            .with_no_client_auth();
        Ok(Self {
            tls: tokio_rustls::TlsConnector::from(Arc::new(tls_config)),
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

            let tcp_stream = tokio::time::timeout(connect_timeout, TcpStream::connect(&authority))
                .await
                .map_err(|_| types::ErrorCode::ConnectionTimeout)?
                .map_err(|e| match e.kind() {
                    std::io::ErrorKind::AddrNotAvailable => {
                        dns_error("address not available".to_string(), 0)
                    }

                    _ => {
                        if e.to_string()
                            .starts_with("failed to lookup address information")
                        {
                            dns_error("address not available".to_string(), 0)
                        } else {
                            types::ErrorCode::ConnectionRefused
                        }
                    }
                })?;

            let mut sender = if use_tls {
                #[cfg(any(target_arch = "riscv64", target_arch = "s390x"))]
                {
                    return Err(types::ErrorCode::InternalError(Some(
                        "unsupported architecture for SSL".to_string(),
                    )));
                }

                #[cfg(not(any(target_arch = "riscv64", target_arch = "s390x")))]
                {
                    use rustls::pki_types::ServerName;

                    let mut parts = authority.split(":");
                    let host = parts.next().unwrap_or(&authority);
                    let domain = ServerName::try_from(host)
                        .map_err(|err| {
                            warn!(?err, "DNS lookup failed");
                            dns_error("invalid DNS name".to_string(), 0)
                        })?
                        .to_owned();
                    let stream = self.tls.connect(domain, tcp_stream).await.map_err(|err| {
                        warn!(?err, "TLS protocol error");
                        types::ErrorCode::TlsProtocolError
                    })?;
                    let stream = TokioIo::new(stream);

                    let (sender, conn) = tokio::time::timeout(
                        connect_timeout,
                        hyper::client::conn::http1::handshake(stream),
                    )
                    .await
                    .map_err(|_| types::ErrorCode::ConnectionTimeout)?
                    .map_err(hyper_request_error)?;
                    spawn(conn);
                    sender
                }
            } else {
                let tcp_stream = TokioIo::new(tcp_stream);
                let (sender, conn) = tokio::time::timeout(
                    connect_timeout,
                    hyper::client::conn::http1::handshake(tcp_stream),
                )
                .await
                .map_err(|_| types::ErrorCode::ConnectionTimeout)?
                .map_err(hyper_request_error)?;
                spawn(conn);
                sender
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

            debug!(uri = ?request.uri(), "sending HTTP request");
            let res = tokio::time::timeout(first_byte_timeout, sender.send_request(request))
                .instrument(tracing::debug_span!("http_request"))
                .await
                .map_err(|_| types::ErrorCode::ConnectionReadTimeout)?
                .map_err(hyper_request_error)?
                .map(|body| {
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
                });
            trace!("HTTP response received");
            Ok(res)
        }
        .await)
    }
}

/// Handle provider control commands
impl Provider for HttpClientProvider {}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use anyhow::Result;
    use http::Request;
    use wrpc_interface_http::{HttpBody, ServeOutgoingHandlerHttp};

    use crate::HttpClientProvider;

    #[ignore = "requires network access"]
    #[tokio::test]
    async fn test_client() -> Result<()> {
        let client = HttpClientProvider::new(&HashMap::new()).await.unwrap();
        let request = Request::builder()
            .method("POST")
            .uri("https://www.google-analytics.com/g/collect")
            .header(http::header::HOST, "www.google-analytics.com")
            .body(HttpBody {
                body: Box::pin(futures::stream::empty()),
                trailers: Box::pin(async { None }),
            })?;
        let _ = client.handle(None, request, None).await??;
        Ok(())
    }
}
