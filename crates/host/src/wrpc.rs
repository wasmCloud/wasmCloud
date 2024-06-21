// //! This module provides `wasmcloud`-specific implementations of `wrpc_transport` traits.
// //!
// //! Specifically, we wrap the [`wrpc_transport::Transmitter`], [`wrpc_transport::Invocation`],
// //! and [`wrpc_transport::Client`] traits in order to:
// //! - Propagate trace context
// //! - Append invocation headers
// //! - Perform invocation validation (where necessary)
// //!
// //! Most logic is delegated to the underlying `wrpc_transport_nats` client, which provides the
// //! actual NATS-based transport implementation.

use tokio::io::{AsyncRead, AsyncWrite};

use std::{marker::PhantomData, sync::Arc};
use wrpc_transport::{Invocation, Session};
use wrpc_transport_nats::Client as WrpcClient;

use crate::bindings::wasmcloud;

// This enum is used to differentiate between component export invocations
pub enum InvocationType {
    Custom {
        instance: Arc<String>,
        name: Arc<String>,
    },
    IncomingHttpHandle(http::Request<wasmtime_wasi_http::body::HyperIncomingBody>),
    MessagingHandleMessage(wasmcloud::messaging::types::BrokerMessage),
}

impl std::fmt::Debug for InvocationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InvocationType::Custom { instance, name, .. } => f
                .debug_struct("Custom")
                .field("interface", instance)
                .field("function", name)
                .finish(),
            InvocationType::IncomingHttpHandle(_) => f.debug_tuple("IncomingHttpHandle").finish(),
            InvocationType::MessagingHandleMessage(_) => {
                f.debug_tuple("MessagingHandleMessage").finish()
            }
        }
    }
}

pub struct AcceptedInvocation<O, I, S, IO, II>
where
    O: AsyncWrite + wrpc_transport::Index<IO>,
    I: AsyncRead + wrpc_transport::Index<II>,
    S: Session,
{
    pub context: Option<async_nats::HeaderMap>,
    pub invoke_type: InvocationType,
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
        invoke_type: InvocationType,
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
/// Wrapper around [`wrpc_transport_nats::Client`] that includes a [`async_nats::HeaderMap`] for
/// passing invocation and trace context.
#[derive(Clone, Debug)]
pub struct ClientBuilder {
    nats: Arc<async_nats::Client>,
    lattice: String,
    component_id: String,
}

impl ClientBuilder {
    /// Create a new wRPC [ClientBuilder] with the given NATS client, lattice, component ID, and headers.
    ///
    /// ## Arguments
    /// * `nats` - The NATS client to use for communication.
    /// * `lattice` - The lattice ID to use for communication.
    /// * `component_id` - The component ID to use for communication.
    pub fn new(
        nats: impl Into<Arc<async_nats::Client>>,
        lattice: &str,
        component_id: &str,
    ) -> Self {
        Self {
            nats: nats.into(),
            lattice: lattice.to_string(),
            component_id: component_id.to_string(),
        }
    }

    /// Produces a wRPC client.
    pub fn build(&self) -> WrpcClient {
        WrpcClient::new(
            Arc::clone(&self.nats),
            format!("{}.{}", self.lattice, self.component_id),
        )
    }
}
