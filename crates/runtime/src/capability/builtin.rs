use super::{format_opt, logging};

use core::fmt::Debug;
use core::future::Future;
use core::pin::Pin;

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{instrument, trace};

#[derive(Clone, Default)]
pub struct Handler {
    bus: Option<Arc<dyn Bus + Sync + Send>>,
    logging: Option<Arc<dyn Logging + Sync + Send>>,
    incoming_http: Option<Arc<dyn IncomingHttp + Sync + Send>>,
}

impl Debug for Handler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handler")
            .field("bus", &format_opt(&self.bus))
            .field("logging", &format_opt(&self.logging))
            .finish()
    }
}

impl Handler {
    /// Replace [Bus] handler returning the old one, if such was set
    pub fn replace_bus(
        &mut self,
        bus: Arc<dyn Bus + Send + Sync>,
    ) -> Option<Arc<dyn Bus + Send + Sync>> {
        self.bus.replace(bus)
    }

    /// Replace [`IncomingHttp`] handler returning the old one, if such was set
    pub fn replace_incoming_http(
        &mut self,
        incoming_http: Arc<dyn IncomingHttp + Send + Sync>,
    ) -> Option<Arc<dyn IncomingHttp + Send + Sync>> {
        self.incoming_http.replace(incoming_http)
    }

    /// Replace [Logging] handler returning the old one, if such was set
    pub fn replace_logging(
        &mut self,
        logging: Arc<dyn Logging + Send + Sync>,
    ) -> Option<Arc<dyn Logging + Send + Sync>> {
        self.logging.replace(logging)
    }
}

#[async_trait]
/// `wasmcloud:bus/host` implementation
pub trait Bus {
    /// Handle `wasmcloud:bus/host.call`
    async fn call(
        &self,
        operation: String,
    ) -> anyhow::Result<(
        Box<dyn AsyncWrite + Sync + Send + Unpin>,
        Box<dyn AsyncRead + Sync + Send + Unpin>,
        Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
    )>;
}

#[async_trait]
/// `wasi:http/incoming-handler` implementation
pub trait IncomingHttp {
    /// Handle `wasi:http/incoming-handler`
    async fn handle(
        &self,
        request: http::Request<Box<dyn AsyncRead + Sync + Send + Unpin>>,
    ) -> anyhow::Result<http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>>;
}

#[async_trait]
/// `wasi:logging/logging` implementation
pub trait Logging {
    /// Handle `wasi:logging/logging.log`
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()>;
}

#[async_trait]
impl Bus for Handler {
    #[instrument]
    async fn call(
        &self,
        operation: String,
    ) -> anyhow::Result<(
        Box<dyn AsyncWrite + Sync + Send + Unpin>,
        Box<dyn AsyncRead + Sync + Send + Unpin>,
        Pin<Box<dyn Future<Output = Result<(), String>> + Send>>,
    )> {
        if let Some(ref bus) = self.bus {
            trace!("call `Bus` handler");
            bus.call(operation).await
        } else {
            bail!("host cannot handle `{operation}`")
        }
    }
}

#[async_trait]
impl Logging for Handler {
    #[instrument]
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        if let Some(ref logging) = self.logging {
            trace!("call `Logging` handler");
            logging.log(level, context, message).await
        } else {
            // discard all log invocations by default
            Ok(())
        }
    }
}

#[async_trait]
impl IncomingHttp for Handler {
    #[instrument(skip(request))]
    async fn handle(
        &self,
        request: http::Request<Box<dyn AsyncRead + Sync + Send + Unpin>>,
    ) -> anyhow::Result<http::Response<Box<dyn AsyncRead + Sync + Send + Unpin>>> {
        trace!("call `IncomingHttp` handler");
        self.incoming_http
            .as_ref()
            .context("cannot handle `wasi:http/incoming-handler.handle`")?
            .handle(request)
            .await
    }
}

/// A [Handler] builder used to configure it
#[derive(Clone, Default)]
pub(crate) struct HandlerBuilder {
    /// [`Bus`] handler
    pub bus: Option<Arc<dyn Bus + Sync + Send>>,
    /// [`IncomingHttp`] handler
    pub incoming_http: Option<Arc<dyn IncomingHttp + Sync + Send>>,
    /// [`Logging`] handler
    pub logging: Option<Arc<dyn Logging + Sync + Send>>,
}

impl HandlerBuilder {
    /// Set [`Bus`] handler
    pub fn bus(self, bus: Arc<impl Bus + Sync + Send + 'static>) -> Self {
        Self {
            bus: Some(bus),
            ..self
        }
    }

    /// Set [`IncomingHttp`] handler
    pub fn incoming_http(
        self,
        incoming_http: Arc<impl IncomingHttp + Sync + Send + 'static>,
    ) -> Self {
        Self {
            incoming_http: Some(incoming_http),
            ..self
        }
    }

    /// Set [`Logging`] handler
    pub fn logging(self, logging: Arc<impl Logging + Sync + Send + 'static>) -> Self {
        Self {
            logging: Some(logging),
            ..self
        }
    }
}

impl Debug for HandlerBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HandlerBuilder")
            .field("bus", &format_opt(&self.bus))
            .field("incoming_http", &format_opt(&self.incoming_http))
            .field("logging", &format_opt(&self.logging))
            .finish()
    }
}

impl From<Handler> for HandlerBuilder {
    fn from(
        Handler {
            bus,
            incoming_http,
            logging,
        }: Handler,
    ) -> Self {
        Self {
            bus,
            incoming_http,
            logging,
        }
    }
}

impl From<HandlerBuilder> for Handler {
    fn from(
        HandlerBuilder {
            bus,
            incoming_http,
            logging,
        }: HandlerBuilder,
    ) -> Self {
        Self {
            bus,
            logging,
            incoming_http,
        }
    }
}
