use core::future::Future;
use core::pin::pin;

use anyhow::Context as _;
use futures::StreamExt;
use hyper_util::rt::TokioExecutor;
use tokio::{select, spawn};
use tracing::{debug, error, instrument, warn};
use wasmcloud_provider_sdk::{get_connection, ProviderHandler};
use wrpc_interface_http::try_http_to_outgoing_response;
use wrpc_transport::AcceptedInvocation;

#[instrument(level = "trace", skip_all)]
pub async fn serve_handle<Ctx, Tx, C>(
    client: hyper_util::client::legacy::Client<
        C,
        wrpc_interface_http::IncomingBody<
            wrpc_transport::IncomingInputStream,
            wrpc_interface_http::IncomingFields,
        >,
    >,
    AcceptedInvocation {
        params: (wrpc_interface_http::IncomingRequestHttp(req), opts),
        result_subject,
        transmitter,
        ..
    }: AcceptedInvocation<
        Ctx,
        (
            wrpc_interface_http::IncomingRequestHttp,
            Option<wrpc_interface_http::RequestOptions>,
        ),
        Tx,
    >,
) where
    Tx: wrpc_transport::Transmitter,
    C: hyper_util::client::legacy::connect::Connect + Clone + Send + Sync + 'static,
{
    // TODO: Use opts
    let _ = opts;
    debug!(uri = ?req.uri(), "send HTTP request");
    let res = match client.request(req).await.map(try_http_to_outgoing_response) {
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

/// HTTP client capability provider implementation struct
#[derive(Default, Clone)]
pub struct HttpClientProvider;

/// Handle provider control commands
impl ProviderHandler for HttpClientProvider {}

#[instrument(level = "trace", skip_all)]
pub async fn serve(commands: impl Future<Output = ()>) -> anyhow::Result<()> {
    let connection = get_connection();
    let wrpc = connection.get_wrpc_client(connection.provider_key());
    let http_client = hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(
        hyper_rustls::HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_all_versions()
            .build(),
    );
    let mut commands = pin!(commands);
    'outer: loop {
        use wrpc_interface_http::OutgoingHandler as _;
        let handle_invocations = wrpc
            .serve_handle_http()
            .await
            .context("failed to serve `wrpc:http/outgoing-handler.handle` invocations")?;
        let mut handle_invocations = pin!(handle_invocations);
        loop {
            select! {
                invocation = handle_invocations.next() => {
                    match invocation {
                        Some(Ok(invocation)) => {
                            spawn(serve_handle(http_client.clone(), invocation));
                        },
                        Some(Err(err)) => {
                            error!(?err, "failed to accept `wrpc:http/outgoing-handler.handle` invocation")
                        },
                        None => {
                            warn!("`wrpc:http/outgoing-handler.handle` stream unexpectedly finished, resubscribe");
                            continue 'outer
                        }
                    }
                }
                _ = &mut commands => {
                    debug!("shutdown command received");
                    return Ok(())
                }
            }
        }
    }
}
