pub mod blobstore;
pub mod http;
pub mod keyvalue;

use core::future::Future;

use tower::ServiceExt;
use wrpc_transport::{AcceptedInvocation, IncomingInvocation, OutgoingInvocation};

use crate::provider::invocation_context;
use crate::Context;

pub(crate) struct WrpcContextClient(wasmcloud_core::wrpc::Client);

impl wrpc_transport::Client for WrpcContextClient {
    type Context = Option<Context>;
    type Subject = <wasmcloud_core::wrpc::Client as wrpc_transport::Client>::Subject;
    type Subscriber = <wasmcloud_core::wrpc::Client as wrpc_transport::Client>::Subscriber;
    type Transmission = <wasmcloud_core::wrpc::Client as wrpc_transport::Client>::Transmission;
    type Acceptor = <wasmcloud_core::wrpc::Client as wrpc_transport::Client>::Acceptor;
    type Invocation = <wasmcloud_core::wrpc::Client as wrpc_transport::Client>::Invocation;
    type InvocationStream<Ctx, T, Tx: wrpc_transport::Transmitter> =
        <wasmcloud_core::wrpc::Client as wrpc_transport::Client>::InvocationStream<Ctx, T, Tx>;

    fn serve<Ctx, T, Tx, S, Fut>(
        &self,
        instance: &str,
        name: &str,
        svc: S,
    ) -> impl Future<Output = anyhow::Result<Self::InvocationStream<Ctx, T, Tx>>>
    where
        Tx: wrpc_transport::Transmitter,
        S: tower::Service<
                IncomingInvocation<Self::Context, Self::Subscriber, Self::Acceptor>,
                Future = Fut,
            > + Send
            + Clone
            + 'static,
        Fut: Future<Output = Result<AcceptedInvocation<Ctx, T, Tx>, anyhow::Error>> + Send,
    {
        self.0.serve(
            instance,
            name,
            svc.map_request(
                |IncomingInvocation {
                     context,
                     payload,
                     param_subject,
                     error_subject,
                     handshake_subject,
                     subscriber,
                     acceptor,
                 }: IncomingInvocation<Option<_>, _, _>| {
                    IncomingInvocation {
                        context: context.as_ref().map(invocation_context),
                        payload,
                        param_subject,
                        error_subject,
                        handshake_subject,
                        subscriber,
                        acceptor,
                    }
                },
            ),
        )
    }

    fn new_invocation(
        &self,
    ) -> OutgoingInvocation<Self::Invocation, Self::Subscriber, Self::Subject> {
        self.0.new_invocation()
    }
}
