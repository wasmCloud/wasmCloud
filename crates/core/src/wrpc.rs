//! This module provides `wasmcloud`-specific implementations of [`wrpc_transport_legacy`] traits.
//!
//! Specifically, we wrap the [`wrpc_transport_legacy::Transmitter`], [`wrpc_transport_legacy::Invocation`],
//! and [`wrpc_transport_legacy::LegacyClient`] traits in order to:
//! - Propagate trace context
//! - Append invocation headers
//! - Perform invocation validation (where necessary)
//!
//! Most logic is delegated to the underlying `wrpc_transport_nats_legacy` client, which provides the
//! actual NATS-based transport implementation.
//!
//! [wrpc-transport]: https://docs.rs/wrpc-transport

use core::future::Future;
use core::time::Duration;

use std::{marker::PhantomData, pin::Pin, sync::Arc};

use anyhow::Context as _;
use async_nats::HeaderMap;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tower::ServiceExt;
use tracing::instrument;
use wrpc_transport::{Invocation, Session};
use wrpc_transport_legacy::{
    AcceptedInvocation as LegacyAcceptedInvocation, Client, Encode, IncomingInvocation,
    OutgoingInvocation,
};
use wrpc_transport_nats::SubjectWriter;
use wrpc_transport_nats_legacy::{Subject, Subscriber, Transmission};

/// wRPC interface bindings
mod bindings {
    wit_bindgen_wrpc::generate!();
}
pub use bindings::wasmcloud;

#[derive(Clone)]
pub enum WrpcClient {
    Legacy(LegacyClient),
    Wrpc(wrpc_transport_nats::Client),
}

impl WrpcClient {
    pub fn new(
        is_legacy: bool,
        rpc_nats: Arc<async_nats::Client>,
        lattice: &str,
        id: &str,
    ) -> Self {
        if is_legacy {
            WrpcClient::Legacy(LegacyClient::new(
                rpc_nats,
                lattice,
                id,
                async_nats::HeaderMap::new(),
                Duration::default(),
            ))
        } else {
            WrpcClient::Wrpc(wrpc_transport_nats::Client::new(
                rpc_nats,
                format!("{}.{}", lattice, id),
            ))
        }
    }

    pub fn for_instance(
        instance: &str,
        rpc_nats: Arc<async_nats::Client>,
        lattice: &str,
        id: &str,
    ) -> Self {
        let is_legacy = matches!(
            instance,
            "wasi:http/incoming-handler@0.2.0" | "wasmcloud:messaging/handler@0.2.0"
        );

        WrpcClient::new(is_legacy, rpc_nats, lattice, id)
    }
}

/// Wrapper around [`wrpc_transport_nats_legacy::Transmitter`] that includes a [`async_nats::HeaderMap`] for
/// passing invocation and trace context.
#[derive(Clone, Debug)]
pub struct TransmitterWithHeaders {
    inner: wrpc_transport_nats_legacy::Transmitter,
    headers: HeaderMap,
}

impl TransmitterWithHeaders {
    pub(crate) fn new(
        transmitter: wrpc_transport_nats_legacy::Transmitter,
        headers: HeaderMap,
    ) -> Self {
        Self {
            inner: transmitter,
            headers,
        }
    }
}

impl wrpc_transport_legacy::Transmitter for TransmitterWithHeaders {
    type Subject = Subject;
    type PublishError = wrpc_transport_nats_legacy::PublishError;

    #[instrument(level = "trace", ret, skip(self))]
    async fn transmit(
        &self,
        subject: Self::Subject,
        payload: Bytes,
    ) -> Result<(), Self::PublishError> {
        self.inner
            .transmit_with_headers(subject, self.headers.clone(), payload)
            .await
    }
}

/// Wrapper around [`wrpc_transport_nats_legacy::Invocation`] that includes a [`async_nats::HeaderMap`] for
/// passing invocation and trace context.
pub struct InvocationWithHeaders {
    inner: wrpc_transport_nats_legacy::Invocation,
    headers: HeaderMap,
    timeout: Duration,
}

impl InvocationWithHeaders {
    /// This function just delegates to the underlying [`wrpc_transport_nats_legacy::Invocation::begin`] function,
    /// but since we're consuming `self` it also returns the headers to avoid a clone in [`InvocationWithHeaders::invoke`].
    pub(crate) async fn begin(
        self,
        params: impl wrpc_transport_legacy::Encode,
    ) -> anyhow::Result<(
        wrpc_transport_nats_legacy::InvocationPre,
        HeaderMap,
        Duration,
    )> {
        self.inner
            .begin(params)
            .await
            .map(|inv| (inv, self.headers, self.timeout))
    }
}

impl wrpc_transport_legacy::Invocation for InvocationWithHeaders {
    type Transmission = Transmission;
    type TransmissionFailed = Box<dyn Future<Output = ()> + Send + Unpin>;

    async fn invoke(
        self,
        instance: &str,
        name: &str,
        params: impl Encode,
    ) -> anyhow::Result<(Self::Transmission, Self::TransmissionFailed)> {
        let subject = self.inner.client().static_subject(instance, name);
        let (inv, headers, timeout) = self.begin(params).await?;

        let (tx, tx_failed) =
            tokio::time::timeout(timeout, inv.invoke_with_headers(subject, headers))
                .await
                .context("invocation timed out")??;
        Ok((tx, Box::new(tx_failed)))
    }
}

/// Wrapper around [`wrpc_transport_nats_legacy::Acceptor`] that includes a [`async_nats::HeaderMap`] for
/// passing invocation and trace context.
pub struct AcceptorWithHeaders {
    inner: wrpc_transport_nats_legacy::Acceptor,
    headers: HeaderMap,
}

impl wrpc_transport_legacy::Acceptor for AcceptorWithHeaders {
    type Subject = Subject;
    type Transmitter = TransmitterWithHeaders;

    #[instrument(level = "trace", skip(self))]
    async fn accept(
        self,
        rx: Self::Subject,
    ) -> anyhow::Result<(Self::Subject, Self::Subject, Self::Transmitter)> {
        let (result_subject, error_subject, transmitter) = self
            .inner
            .accept_with_headers(rx, self.headers.clone())
            .await?;
        Ok((
            result_subject,
            error_subject,
            TransmitterWithHeaders::new(transmitter, self.headers),
        ))
    }
}

/// Wrapper around [`wrpc_transport_nats_legacy::LegacyClient`] that includes a [`async_nats::HeaderMap`] for
/// passing invocation and trace context.
#[derive(Clone, Debug)]
pub struct LegacyClient {
    inner: wrpc_transport_nats_legacy::Client,
    headers: HeaderMap,
    timeout: Duration,
}

impl LegacyClient {
    /// Create a new wRPC [LegacyClient] with the given NATS client, lattice, component ID, and headers.
    ///
    /// ## Arguments
    /// * `nats` - The NATS client to use for communication.
    /// * `lattice` - The lattice ID to use for communication.
    /// * `prefix` - The component ID to use for communication.
    /// * `headers` - The headers to include with each outbound invocation.
    pub fn new(
        nats: impl Into<Arc<async_nats::Client>>,
        lattice: &str,
        component_id: &str,
        headers: HeaderMap,
        timeout: Duration,
    ) -> Self {
        Self {
            inner: wrpc_transport_nats_legacy::Client::new(
                nats,
                format!("{lattice}.{component_id}"),
            ),
            headers,
            timeout,
        }
    }

    pub async fn serve_exports(
        &self,
        instance: &str,
    ) -> anyhow::Result<
        Vec<Pin<Box<dyn Stream<Item = Result<WrpcInvocationStream, anyhow::Error>> + Send>>>,
    > {
        // Old export serving
        let mut exports: Vec<Pin<Box<dyn Stream<Item = _> + Send>>> = Vec::new();
        match instance {
            "wasi:http/incoming-handler@0.2.0" => {
                use wrpc_interface_http::IncomingHandler;

                let invocations = self
                    .clone()
                    .serve_handle_wasmtime()
                    .await
                    .context("failed to serve `wrpc:http/incoming-handler.handle`")?;
                exports.push(Box::pin(invocations.map(move |invocation| {
                    invocation.map(
                        |LegacyAcceptedInvocation {
                             context,
                             params,
                             result_subject,
                             error_subject,
                             transmitter,
                         }| {
                            LegacyAcceptedInvocation {
                                context,
                                params: InvocationParams::IncomingHttpHandle(params.0),
                                result_subject,
                                error_subject,
                                transmitter,
                            }
                            .into()
                        },
                    )
                })));
            }
            "wasmcloud:messaging/handler@0.2.0" => {
                let invocations = self
                    .clone()
                    .serve_static(instance, "handle-message")
                    .await
                    .context("failed to serve `wasmcloud:messaging/handler.handle-message`")?;
                exports.push(Box::pin(invocations.map(move |invocation| {
                    invocation.map(
                        |LegacyAcceptedInvocation {
                             context,
                             params,
                             result_subject,
                             error_subject,
                             transmitter,
                         }| {
                            LegacyAcceptedInvocation {
                                context,
                                params: InvocationParams::MessagingHandleMessage(params),
                                result_subject,
                                error_subject,
                                transmitter,
                            }
                            .into()
                        },
                    )
                })));
            }
            _ => return Err(anyhow::anyhow!("Unsupported instance type")),
        }

        Ok(exports)
    }
}

impl wrpc_transport_legacy::Client for LegacyClient {
    type Context = Option<HeaderMap>;
    type Subject = Subject;
    type Subscriber = Subscriber;
    type Transmission = Transmission;
    type Acceptor = AcceptorWithHeaders;
    type Invocation = InvocationWithHeaders;
    type InvocationStream<Ctx, T, Tx: wrpc_transport_legacy::Transmitter> =
        <wrpc_transport_nats_legacy::Client as wrpc_transport_legacy::Client>::InvocationStream<
            Ctx,
            T,
            Tx,
        >;

    #[instrument(level = "trace", skip(self, svc))]
    fn serve<Ctx, T, Tx, S, Fut>(
        &self,
        instance: &str,
        name: &str,
        svc: S,
    ) -> impl Future<Output = anyhow::Result<Self::InvocationStream<Ctx, T, Tx>>> + Send
    where
        Tx: wrpc_transport_legacy::Transmitter,
        S: tower::Service<
                IncomingInvocation<Self::Context, Self::Subscriber, Self::Acceptor>,
                Future = Fut,
            > + Send
            + Clone
            + 'static,
        Fut: Future<Output = anyhow::Result<LegacyAcceptedInvocation<Ctx, T, Tx>>> + Send,
    {
        self.inner.serve(
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
                 }: IncomingInvocation<Self::Context, _, _>| {
                    IncomingInvocation {
                        context: context.clone(),
                        payload,
                        param_subject,
                        error_subject,
                        handshake_subject,
                        subscriber,
                        acceptor: AcceptorWithHeaders {
                            inner: acceptor,
                            headers: context.unwrap_or_default(),
                        },
                    }
                },
            ),
        )
    }

    fn new_invocation(
        &self,
    ) -> OutgoingInvocation<Self::Invocation, Self::Subscriber, Self::Subject> {
        let transport_invocation = self.inner.new_invocation();
        let invocation_with_headers = InvocationWithHeaders {
            inner: transport_invocation.invocation,
            headers: self.headers.clone(),
            timeout: self.timeout,
        };
        OutgoingInvocation {
            invocation: invocation_with_headers,
            subscriber: transport_invocation.subscriber,
            result_subject: transport_invocation.result_subject,
            error_subject: transport_invocation.error_subject,
        }
    }
}

pub enum WrpcInvocationStream {
    Wrpc(
        AcceptedInvocation<
            SubjectWriter,
            wrpc_transport_nats::Reader,
            wrpc_transport_nats::Session<SubjectWriter>,
            SubjectWriter,
            wrpc_transport_nats::Reader,
        >,
    ),
    Legacy(LegacyAcceptedInvocation<Option<HeaderMap>, InvocationParams, TransmitterWithHeaders>),
}

impl
    From<
        AcceptedInvocation<
            SubjectWriter,
            wrpc_transport_nats::Reader,
            wrpc_transport_nats::Session<SubjectWriter>,
            SubjectWriter,
            wrpc_transport_nats::Reader,
        >,
    > for WrpcInvocationStream
{
    fn from(
        invocation: AcceptedInvocation<
            SubjectWriter,
            wrpc_transport_nats::Reader,
            wrpc_transport_nats::Session<SubjectWriter>,
            SubjectWriter,
            wrpc_transport_nats::Reader,
        >,
    ) -> Self {
        WrpcInvocationStream::Wrpc(invocation)
    }
}

impl From<LegacyAcceptedInvocation<Option<HeaderMap>, InvocationParams, TransmitterWithHeaders>>
    for WrpcInvocationStream
{
    fn from(
        invocation: LegacyAcceptedInvocation<
            Option<HeaderMap>,
            InvocationParams,
            TransmitterWithHeaders,
        >,
    ) -> Self {
        WrpcInvocationStream::Legacy(invocation)
    }
}
pub struct AcceptedInvocation<O, I, S, IO, II>
where
    O: AsyncWrite + wrpc_transport::Index<IO>,
    I: AsyncRead + wrpc_transport::Index<II>,
    S: Session,
{
    pub context: Option<async_nats::HeaderMap>,
    pub invoke_type: InvocationParams,
    pub invocation: Invocation<O, I, S>,
    _marker: PhantomData<(IO, II)>,
}

impl<O, I, S, IO, II> AcceptedInvocation<O, I, S, IO, II>
where
    O: AsyncWrite + wrpc_transport::Index<IO>,
    I: AsyncRead + wrpc_transport::Index<II>,
    S: Session,
{
    pub fn new(
        context: Option<async_nats::HeaderMap>,
        invoke_type: InvocationParams,
        invocation: Invocation<O, I, S>,
    ) -> Self {
        Self {
            context,
            invoke_type,
            invocation,
            _marker: PhantomData,
        }
    }
}

// This enum is used to differentiate between component export invocations
pub enum InvocationParams {
    Custom {
        instance: Arc<String>,
        name: Arc<String>,
    },
    IncomingHttpHandle(http::Request<wasmtime_wasi_http::body::HyperIncomingBody>),
    MessagingHandleMessage(wasmcloud::messaging::types::BrokerMessage),
}

impl std::fmt::Debug for InvocationParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InvocationParams::Custom { instance, name, .. } => f
                .debug_struct("Custom")
                .field("interface", instance)
                .field("function", name)
                .finish(),
            InvocationParams::IncomingHttpHandle(_) => f.debug_tuple("IncomingHttpHandle").finish(),
            InvocationParams::MessagingHandleMessage(_) => {
                f.debug_tuple("MessagingHandleMessage").finish()
            }
        }
    }
}
