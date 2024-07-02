use std::collections::HashMap;

use futures::StreamExt;
use hyper_util::rt::TokioExecutor;
use tokio::spawn;
use tracing::{debug, error, instrument, Instrument};

use wasmcloud_provider_sdk::core::tls;
use wasmcloud_provider_sdk::interfaces::http::OutgoingHandler;
use wasmcloud_provider_sdk::{propagate_trace_for_ctx, Context, Provider};
use wrpc_interface_http::try_http_to_outgoing_response;
use wrpc_transport_legacy::AcceptedInvocation;

/// HTTP client capability provider implementation struct
#[derive(Clone)]
pub struct HttpClientProvider {
    client: hyper_util::client::legacy::Client<
        hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
        wrpc_interface_http::IncomingBody<
            wrpc_transport_legacy::IncomingInputStream,
            wrpc_interface_http::IncomingFields,
        >,
    >,
}

const DEFAULT_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
// Configuration
const LOAD_NATIVE_CERTS: &str = "load_native_certs";
const LOAD_WEBPKI_CERTS: &str = "load_webpki_certs";
const SSL_CERTS_FILE: &str = "ssl_certs_file";

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

impl OutgoingHandler for HttpClientProvider {
    #[instrument(level = "debug", skip_all)]
    async fn serve_handle<Tx: wrpc_transport_legacy::Transmitter>(
        &self,
        AcceptedInvocation {
            context,
            params: (wrpc_interface_http::IncomingRequestHttp(mut req), opts),
            result_subject,
            transmitter,
            ..
        }: AcceptedInvocation<
            Option<Context>,
            (
                wrpc_interface_http::IncomingRequestHttp,
                Option<wrpc_interface_http::RequestOptions>,
            ),
            Tx,
        >,
    ) {
        propagate_trace_for_ctx!(context);

        // TODO: Use opts
        let _ = opts;
        debug!(uri = ?req.uri(), "send HTTP request");
        // Ensure we have a User-Agent header set.
        req.headers_mut()
            .entry(http::header::USER_AGENT)
            .or_insert(http::header::HeaderValue::from_static(DEFAULT_USER_AGENT));
        let res = match self
            .client
            .request(req)
            .instrument(tracing::debug_span!("http_request"))
            .await
            .map(try_http_to_outgoing_response)
        {
            Ok(Ok((res, errors))) => {
                debug!("received HTTP response");
                // TODO: Handle body errors better
                spawn(errors.for_each(|err| async move { error!(?err, "body error encountered") }));
                Ok(res)
            }
            Ok(Err(err)) => {
                error!(
                    ?err,
                    "failed to convert `http` response to `wrpc:http` response"
                );
                return;
            }
            Err(err) => {
                debug!(?err, "failed to send HTTP request");
                Err(wrpc_interface_http::ErrorCode::InternalError(Some(
                    err.to_string(),
                )))
            }
        };
        if let Err(err) = transmitter.transmit_static(result_subject, res).await {
            error!(?err, "failed to transmit response");
        }
    }
}

/// Handle provider control commands
impl Provider for HttpClientProvider {}
