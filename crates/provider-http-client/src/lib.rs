use core::convert::Infallible;
use core::pin::pin;

use std::collections::HashMap;

use anyhow::Context as _;
use bytes::Bytes;
use futures::StreamExt as _;
use http_body::Frame;
use http_body_util::{BodyExt as _, StreamBody};
use hyper_util::rt::TokioExecutor;
use tokio::task::JoinSet;
use tokio::{select, spawn};
use tracing::{debug, error, instrument, trace, warn, Instrument};

use wasmcloud_provider_sdk::core::tls;
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, load_host_data, propagate_trace_for_ctx,
    run_provider, Context, Provider,
};
use wrpc_interface_http::{
    split_outgoing_http_body, try_fields_to_header_map, ServeHttp, ServeOutgoingHandlerHttp,
};

/// HTTP client capability provider implementation struct
#[derive(Clone)]
pub struct HttpClientProvider {
    client: hyper_util::client::legacy::Client<
        hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
        wrpc_interface_http::HttpBody,
    >,
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
                client: hyper_util::client::legacy::Client::builder(TokioExecutor::new())
                    .build(tls::DEFAULT_HYPER_CONNECTOR.clone()),
            });
        }

        let mut ca = rustls::RootCertStore::empty();

        // Load native certificates
        if config
            .get(LOAD_NATIVE_CERTS)
            .map(|v| v.to_ascii_lowercase() == "true")
            .unwrap_or(true)
        {
            let (added, ignored) = ca.add_parsable_certificates(tls::NATIVE_ROOTS.iter().cloned());
            tracing::debug!(added, ignored, "loaded native root certificate store");
        }

        // Load Mozilla trusted root certificates
        if config
            .get(LOAD_WEBPKI_CERTS)
            .map(|v| v.to_ascii_lowercase() == "true")
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
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config(tls_config)
            .https_or_http()
            .enable_all_versions()
            .build();

        Ok(Self {
            client: hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(https),
        })
    }
}

impl ServeOutgoingHandlerHttp<Option<Context>> for HttpClientProvider {
    #[instrument(level = "debug", skip_all)]
    async fn handle(
        &self,
        cx: Option<Context>,
        mut request: http::Request<wrpc_interface_http::HttpBody>,
        options: Option<wrpc_interface_http::bindings::wrpc::http::types::RequestOptions>,
    ) -> anyhow::Result<
        Result<
            http::Response<impl http_body::Body<Data = Bytes, Error = Infallible> + Send + 'static>,
            wrpc_interface_http::bindings::wasi::http::types::ErrorCode,
        >,
    > {
        propagate_trace_for_ctx!(cx);
        wasmcloud_provider_sdk::wasmcloud_tracing::http::HeaderInjector(request.headers_mut())
            .inject_context();

        // TODO: Use opts
        let _ = options;
        // Ensure we have a User-Agent header set.
        request
            .headers_mut()
            .entry(http::header::USER_AGENT)
            .or_insert(http::header::HeaderValue::from_static(DEFAULT_USER_AGENT));
        Ok(async {
            debug!(uri = ?request.uri(), "sending HTTP request");
            let res = self
                .client
                .request(request)
                .instrument(tracing::debug_span!("http_request"))
                .await
                .map_err(|err| {
                    // TODO: Convert error
                    wrpc_interface_http::bindings::wasi::http::types::ErrorCode::InternalError(
                        Some(err.to_string()),
                    )
                })?;
            trace!("HTTP response received");
            Ok(res.map(|body| {
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
            }))
        }
        .await)
    }
}

/// Handle provider control commands
impl Provider for HttpClientProvider {}
