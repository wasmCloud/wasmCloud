//! This module provides `wasmcloud`-specific implementations of `wrpc_transport` traits.
//!
//! Specifically, we wrap the [`wrpc_transport::Transmitter`], [`wrpc_transport::Invocation`],
//! and [`wrpc_transport::Client`] traits in order to:
//! - Propagate trace context
//! - Append invocation headers
//! - Perform invocation validation (where necessary)
//!
//! Most logic is delegated to the underlying `wrpc_transport_nats` client, which provides the
//! actual NATS-based transport implementation.

use core::pin::Pin;

use std::sync::Arc;

use async_nats::HeaderMap;
use bytes::Bytes;
use futures::{Future, Stream, StreamExt};
use tracing::instrument;
use wrpc_transport::{AcceptedInvocation, Encode, IncomingInvocation, OutgoingInvocation};
use wrpc_transport_nats::{InvocationSubscription, Subject, Subscriber, Transmission};

/// Wrapper around [wrpc_transport_nats::Transmitter] that includes a [async_nats::HeaderMap] for
/// passing invocation and trace context.
#[derive(Clone, Debug)]
pub struct TransmitterWithHeaders {
    inner: wrpc_transport_nats::Transmitter,
    headers: HeaderMap,
}

impl TransmitterWithHeaders {
    pub(crate) fn new(transmitter: wrpc_transport_nats::Transmitter, headers: HeaderMap) -> Self {
        Self {
            inner: transmitter,
            headers,
        }
    }
}

impl wrpc_transport::Transmitter for TransmitterWithHeaders {
    type Subject = Subject;
    type PublishError = wrpc_transport_nats::PublishError;

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

/// Wrapper around [wrpc_transport_nats::Invocation] that includes a [async_nats::HeaderMap] for
/// passing invocation and trace context.
pub struct InvocationWithHeaders {
    inner: wrpc_transport_nats::Invocation,
    headers: HeaderMap,
}

impl InvocationWithHeaders {
    /// This function just delegates to the underlying [wrpc_transport_nats::Invocation::begin] function,
    /// but since we're consuming `self` it also returns the headers to avoid a clone in [InvocationWithHeaders::invoke].
    pub(crate) async fn begin(
        self,
        params: impl wrpc_transport::Encode,
    ) -> anyhow::Result<(wrpc_transport_nats::InvocationPre, HeaderMap)> {
        self.inner
            .begin(params)
            .await
            .map(|inv| (inv, self.headers))
    }
}

impl wrpc_transport::Invocation for InvocationWithHeaders {
    type Transmission = Transmission;
    type TransmissionFailed = Box<dyn Future<Output = ()> + Send + Unpin>;

    async fn invoke(
        self,
        instance: &str,
        name: &str,
        params: impl Encode,
    ) -> anyhow::Result<(Self::Transmission, Self::TransmissionFailed)> {
        let subject = self.inner.client().static_subject(instance, name);
        let (inv, headers) = self.begin(params).await?;

        let (tx, tx_failed) = inv.invoke_with_headers(subject, headers).await?;
        Ok((tx, Box::new(tx_failed)))
    }
}

/// Wrapper around [wrpc_transport_nats::Acceptor] that includes a [async_nats::HeaderMap] for
/// passing invocation and trace context.
pub struct AcceptorWithHeaders {
    inner: wrpc_transport_nats::Acceptor,
    headers: HeaderMap,
}

impl wrpc_transport::Acceptor for AcceptorWithHeaders {
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

/// Wrapper around [`wrpc_transport_nats::Client`] that includes a [`async_nats::HeaderMap`] for
/// passing invocation and trace context.
#[derive(Debug, Clone)]
pub struct Client {
    inner: wrpc_transport_nats::Client,
    headers: HeaderMap,
}

impl Client {
    /// Create a new wRPC [Client] with the given NATS client, lattice, component ID, and headers.
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
    ) -> Self {
        Self {
            inner: wrpc_transport_nats::Client::new(nats, format!("{lattice}.{component_id}")),
            headers,
        }
    }
}

impl wrpc_transport::Client for Client {
    type Context = Option<HeaderMap>;
    type Subject = Subject;
    type Subscriber = Subscriber;
    type Transmission = Transmission;
    type Acceptor = AcceptorWithHeaders;
    type Invocation = InvocationWithHeaders;
    type InvocationStream<T> = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<
                        AcceptedInvocation<Option<HeaderMap>, T, TransmitterWithHeaders>,
                    >,
                > + Send,
        >,
    >;

    #[instrument(level = "trace", skip(self, svc))]
    async fn serve<T, S, Fut>(
        &self,
        instance: &str,
        name: &str,
        svc: S,
    ) -> anyhow::Result<Self::InvocationStream<T>>
    where
        S: tower::Service<
                IncomingInvocation<Option<HeaderMap>, Subscriber, AcceptorWithHeaders>,
                Future = Fut,
            > + Send
            + Clone
            + 'static,
        Fut: Future<
                Output = anyhow::Result<
                    AcceptedInvocation<Option<HeaderMap>, T, TransmitterWithHeaders>,
                >,
            > + Send,
    {
        // Map invocations in the stream to construct an `IncomingInvocation` and `Acceptor` with headers.
        let invocations = self
            .inner
            .subscribe(instance, name)
            .await
            .map(InvocationSubscription::into_invocations)?;
        Ok(Box::pin(invocations.then({
            move |invocation| {
                let mut svc = svc.clone();
                async move {
                    let IncomingInvocation {
                        context,
                        payload,
                        param_subject,
                        error_subject,
                        handshake_subject,
                        subscriber,
                        acceptor,
                    } = invocation?;
                    svc.call(IncomingInvocation {
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
                    })
                    .await
                }
            }
        })))
    }

    fn new_invocation(
        &self,
    ) -> OutgoingInvocation<Self::Invocation, Self::Subscriber, Self::Subject> {
        let transport_invocation = self.inner.new_invocation();
        let invocation_with_headers = InvocationWithHeaders {
            inner: transport_invocation.invocation,
            headers: self.headers.clone(),
        };
        OutgoingInvocation {
            invocation: invocation_with_headers,
            subscriber: transport_invocation.subscriber,
            result_subject: transport_invocation.result_subject,
            error_subject: transport_invocation.error_subject,
        }
    }
}
