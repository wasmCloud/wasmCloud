use futures::StreamExt;
use hyper_util::rt::TokioExecutor;
use tokio::spawn;
use tracing::{debug, error, instrument};

use wasmcloud_provider_sdk::core::tls;
use wasmcloud_provider_sdk::interfaces::http::OutgoingHandler;
use wasmcloud_provider_sdk::{propagate_trace_for_ctx, Context, Provider};
use wrpc_interface_http::try_http_to_outgoing_response;
use wrpc_transport::AcceptedInvocation;

/// HTTP client capability provider implementation struct
#[derive(Clone)]
pub struct HttpClientProvider {
    client: hyper_util::client::legacy::Client<
        hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
        wrpc_interface_http::IncomingBody<
            wrpc_transport::IncomingInputStream,
            wrpc_interface_http::IncomingFields,
        >,
    >,
}

const DEFAULT_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

impl Default for HttpClientProvider {
    fn default() -> Self {
        Self {
            client: hyper_util::client::legacy::Client::builder(TokioExecutor::new())
                .build(tls::DEFAULT_HYPER_CONNECTOR.clone()),
        }
    }
}

impl OutgoingHandler for HttpClientProvider {
    #[instrument(level = "trace", skip_all)]
    async fn serve_handle<Tx: wrpc_transport::Transmitter>(
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
