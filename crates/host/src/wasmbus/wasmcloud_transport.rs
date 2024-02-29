//! This module provides `wasmcloud`-specific implementations of `wrpc_transport` traits.
//! Specifically, we wrap the [wrpc_transport::Transmitter], [wrpc_transport::Invocation],
//! and [wrpc_transport::Client] traits in order to pass invocations with headers, which is how
//! we propagate trace context and named configuration information along with the invocation.
//! Most logic is delegated to the underlying `wrpc_transport_nats` client, which provides the
//! actual NATS-based transport implementation.

use std::{pin::Pin, sync::Arc};

use async_nats::HeaderMap;
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Future, Stream};
use tracing::instrument;
use wrpc_transport::{Encode, IncomingInvocation, OutgoingInvocation};
use wrpc_transport_nats::{Acceptor, Subject, Subscriber, Transmission};

/// Wrapper around [wrpc_transport_nats::Transmitter] that includes headers. Lifetime specifier
/// is useful in order to be able to use a reference to the underlying transmitter.
pub(crate) struct TransmitterWithHeaders<'a> {
    inner: &'a wrpc_transport_nats::Transmitter,
    headers: HeaderMap,
}

impl<'a> TransmitterWithHeaders<'a> {
    pub(crate) fn new(
        transmitter: &'a wrpc_transport_nats::Transmitter,
        headers: HeaderMap,
    ) -> Self {
        Self {
            inner: transmitter,
            headers,
        }
    }
}

#[async_trait]
impl<'a> wrpc_transport::Transmitter for TransmitterWithHeaders<'a> {
    type Subject = Subject;
    type PublishError = async_nats::PublishError;

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

pub(crate) struct InvocationWithHeaders {
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

#[derive(Debug, Clone)]
pub(crate) struct Client {
    inner: wrpc_transport_nats::Client,
    headers: HeaderMap,
}

impl Client {
    pub(crate) fn new(
        nats: impl Into<Arc<async_nats::Client>>,
        lattice: String,
        headers: HeaderMap,
    ) -> Self {
        Self {
            inner: wrpc_transport_nats::Client::new(nats, lattice),
            headers,
        }
    }
}

#[async_trait]
impl wrpc_transport::Client for Client {
    type Context = Option<HeaderMap>;
    type Subject = Subject;
    type Subscriber = Subscriber;
    type Transmission = Transmission;
    type Acceptor = Acceptor;
    type InvocationStream = Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<
                        IncomingInvocation<
                            Self::Context,
                            Self::Subject,
                            Self::Subscriber,
                            Self::Acceptor,
                        >,
                    >,
                > + Send,
        >,
    >;
    type Invocation = InvocationWithHeaders;

    #[instrument(level = "trace", skip(self))]
    async fn serve(&self, instance: &str, func: &str) -> anyhow::Result<Self::InvocationStream> {
        self.inner.serve(instance, func).await
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
