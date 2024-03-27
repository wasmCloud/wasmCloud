use core::future::Future;
use core::pin::pin;

use anyhow::Context as _;
use futures::StreamExt as _;
use tokio::{select, spawn};
use tracing::{debug, error, instrument, warn};
use wrpc_interface_http::{IncomingRequestHttp, RequestOptions};
use wrpc_transport::{AcceptedInvocation, Transmitter};

use crate::{get_connection, Context};

use super::WrpcContextClient;

pub trait OutgoingHandler: Send {
    fn serve_handle<Tx: Transmitter + Send>(
        &self,
        invocation: AcceptedInvocation<
            Option<Context>,
            (IncomingRequestHttp, Option<RequestOptions>),
            Tx,
        >,
    ) -> impl Future<Output = ()> + Send;
}

#[instrument(level = "debug", skip_all)]
pub async fn serve_outgoing_handler(
    provider: impl OutgoingHandler + Clone + 'static,
    commands: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    let connection = get_connection();
    let wrpc = WrpcContextClient(connection.get_wrpc_client(connection.provider_key()));
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
                            let provider = provider.clone();
                            spawn(async move { provider.serve_handle(invocation).await });
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
