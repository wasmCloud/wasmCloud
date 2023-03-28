/// Builtin logging capabilities available within `wasmcloud:builtin:logging` namespace
pub mod logging;
/// Builtin random number generation capabilities available within `wasmcloud:builtin:numbergen` namespace
pub mod numbergen;

pub use logging::{DiscardLogging, Invocation as LoggingInvocation, LogLogging};
pub use numbergen::{Invocation as NumbergenInvocation, RandNumbergen};

use core::fmt::Debug;
use core::future::Future;
use core::ops::Deref;

use std::sync::Arc;

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use tracing::instrument;
use wascap::jwt;

#[async_trait]
/// Capability provider invocation handler
pub trait Handle<T>: Sync + Send {
    /// Handles an capability provider invocation
    ///
    /// # Errors
    ///
    /// Returns an [`anyhow::Error`] in case an error is non-recoverable, for example if an invalid
    /// payload is passed to a builtin provider, which will cause an exception in the guest.
    /// Innermost result represents the underlying operation result, which will be passed to the
    /// guest as an application-layer error.
    async fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        binding: String,
        invocation: T,
        call_context: &Option<Vec<u8>>,
    ) -> Result<Option<Vec<u8>>>;
}

/// A handler, which handles all builtin capability invocations offloads all host call capabilities to an arbitrary [`HostInvocation`] handler.
pub struct Handler<H, L = LogLogging, N = RandNumbergen>
where
    H: Handle<HostInvocation>,
    L: Handle<LoggingInvocation>,
    N: Handle<NumbergenInvocation>,
{
    /// Host capability provider invocation handler, using which all non-builtin calls will be handled
    pub host: H,

    /// Logging capability provider invocation handler, using which all known `wasmcloud:builtin:logging` operations will be handled
    pub logging: L,

    /// Random number generator capability provider invocation handler, using which all known `wasmcloud:builtin:numbergen` operations will be handled
    pub numbergen: N,
}

impl<H> Handler<H, LogLogging, RandNumbergen>
where
    H: Handle<HostInvocation> + 'static,
{
    /// Creates a new invocation handler with preset defaults
    #[allow(clippy::new_ret_no_self)]
    pub fn new(host: H) -> Arc<Box<dyn Handle<Invocation>>> {
        HandlerBuilder::new(host).build()
    }
}

impl<H, L, N> From<Handler<H, L, N>> for Arc<Box<dyn Handle<Invocation>>>
where
    H: Handle<HostInvocation> + 'static,
    L: Handle<LoggingInvocation> + 'static,
    N: Handle<NumbergenInvocation> + 'static,
{
    fn from(handler: Handler<H, L, N>) -> Self {
        Arc::new(Box::new(handler))
    }
}

/// A builder for [`Handler`]
pub struct HandlerBuilder<H, L = LogLogging, N = RandNumbergen>
where
    H: Handle<HostInvocation>,
    L: Handle<LoggingInvocation>,
    N: Handle<NumbergenInvocation>,
{
    /// Host call capability provider, using which all non-builtin calls will be handled
    pub host: H,

    /// Logging capability provider, using which all known `wasmcloud:builtin:logging` operations will be handled
    pub logging: L,

    /// Random number generator capability provider, using which all known `wasmcloud:builtin:numbergen` operations will be handled
    pub numbergen: N,
}

impl<H> HandlerBuilder<H, LogLogging, RandNumbergen>
where
    H: Handle<HostInvocation>,
{
    /// Creates a new [`Handler`] builder with preset defaults
    pub fn new(host: H) -> Self {
        Self {
            host,
            logging: LogLogging::default(),
            numbergen: RandNumbergen::default(),
        }
    }
}

impl<H, L, N> HandlerBuilder<H, L, N>
where
    H: Handle<HostInvocation> + 'static,
    L: Handle<LoggingInvocation> + 'static,
    N: Handle<NumbergenInvocation> + 'static,
{
    /// Set [`LoggingInvocation`] handler
    pub fn logging<T>(self, logging: T) -> HandlerBuilder<H, T, N>
    where
        T: Handle<LoggingInvocation>,
    {
        HandlerBuilder {
            logging,
            numbergen: self.numbergen,
            host: self.host,
        }
    }

    /// Set [`NumbergenInvocation`] handler
    pub fn numbergen<T>(self, numbergen: T) -> HandlerBuilder<H, L, T>
    where
        T: Handle<NumbergenInvocation>,
    {
        HandlerBuilder {
            numbergen,
            logging: self.logging,
            host: self.host,
        }
    }

    /// Set [`HostInvocation`] (non-builtin) handler
    pub fn host<T>(self, host: T) -> HandlerBuilder<T, L, N>
    where
        T: Handle<HostInvocation>,
    {
        HandlerBuilder {
            host,
            numbergen: self.numbergen,
            logging: self.logging,
        }
    }

    /// Turns this builder into an invocation handler
    pub fn build(self) -> Arc<Box<dyn Handle<Invocation>>> {
        Arc::new(Box::new(Handler {
            logging: self.logging,
            numbergen: self.numbergen,
            host: self.host,
        }))
    }
}

impl<H, L, N> From<HandlerBuilder<H, L, N>> for Arc<Box<dyn Handle<Invocation>>>
where
    H: Handle<HostInvocation> + 'static,
    L: Handle<LoggingInvocation> + 'static,
    N: Handle<NumbergenInvocation> + 'static,
{
    fn from(builder: HandlerBuilder<H, L, N>) -> Self {
        builder.build()
    }
}

#[async_trait]
impl<T: ?Sized + Handle<HostInvocation>> Handle<HostInvocation> for Box<T> {
    async fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        binding: String,
        invocation: HostInvocation,
        call_context: &Option<Vec<u8>>,
    ) -> Result<Option<Vec<u8>>> {
        self.handle(claims, binding, invocation, call_context).await
    }
}

#[async_trait]
impl<T: ?Sized + Handle<HostInvocation>> Handle<HostInvocation> for Arc<T> {
    async fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        binding: String,
        invocation: HostInvocation,
        call_context: &Option<Vec<u8>>,
    ) -> Result<Option<Vec<u8>>> {
        self.handle(claims, binding, invocation, call_context).await
    }
}

#[async_trait]
impl<T: ?Sized + Handle<HostInvocation>> Handle<HostInvocation> for &T {
    async fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        binding: String,
        invocation: HostInvocation,
        call_context: &Option<Vec<u8>>,
    ) -> Result<Option<Vec<u8>>> {
        self.handle(claims, binding, invocation, call_context).await
    }
}

#[async_trait]
impl<T: ?Sized + Handle<HostInvocation>> Handle<HostInvocation> for &mut T {
    async fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        binding: String,
        invocation: HostInvocation,
        call_context: &Option<Vec<u8>>,
    ) -> Result<Option<Vec<u8>>> {
        self.handle(claims, binding, invocation, call_context).await
    }
}

/// Host capability provider invocation
#[derive(Clone, Debug)]
pub struct HostInvocation {
    /// Capability provider invocation namespace
    pub namespace: String,
    /// Capability provider invocation operation
    pub operation: String,
    /// Capability provider invocation payload
    pub payload: Option<Vec<u8>>,
}

/// A capability provider invocation issued by the [Actor](crate::Actor)
#[derive(Clone, Debug)]
pub enum Invocation {
    /// Builtin logging capability provider invocation
    Logging(LoggingInvocation),
    /// Builtin numbergen capability provider invocation
    Numbergen(NumbergenInvocation),
    /// Host capability provider invocation
    Host(HostInvocation),
}

impl TryFrom<(String, String, Option<Vec<u8>>)> for Invocation {
    type Error = anyhow::Error;

    fn try_from(
        (namespace, operation, payload): (String, String, Option<Vec<u8>>),
    ) -> Result<Self> {
        match (namespace.as_str(), operation.as_str()) {
            ("wasmcloud:builtin:logging", operation) => (operation, payload)
                .try_into()
                .context("failed to parse logging invocation")
                .map(Invocation::Logging),
            ("wasmcloud:builtin:numbergen", operation) => (operation, payload)
                .try_into()
                .context("failed to parse numbergen invocation")
                .map(Invocation::Numbergen),
            (namespace, _) if namespace.starts_with("wasmcloud:builtin:") => {
                bail!("unknown builtin namespace: `{namespace}`")
            }
            _ => Ok(Invocation::Host(HostInvocation {
                namespace,
                operation,
                payload,
            })),
        }
    }
}

#[async_trait]
impl<H, L, N> Handle<Invocation> for Handler<H, L, N>
where
    H: Handle<HostInvocation>,
    L: Handle<LoggingInvocation>,
    N: Handle<NumbergenInvocation>,
{
    #[instrument(skip(self))]
    async fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        binding: String,
        invocation: Invocation,
        call_context: &Option<Vec<u8>>,
    ) -> Result<Option<Vec<u8>>> {
        match invocation {
            Invocation::Logging(invocation) => self
                .logging
                .handle(claims, binding, invocation, call_context)
                .await
                .context("failed to handle logging invocation"),
            Invocation::Numbergen(invocation) => self
                .numbergen
                .handle(claims, binding, invocation, call_context)
                .await
                .context("failed to handle numbergen invocation"),
            Invocation::Host(invocation) => self
                .host
                .handle(claims, binding, invocation, call_context)
                .await
                .context("failed to handle host invocation"),
        }
    }
}

/// A [`HostInvocation`] handler, which wraps an asynchronous function.
///
/// Note, the wrapped function takes [`jwt::Claims`] by value due to claim borrow lifetime issues in
/// async scenario ([`HandlerFuncSync`] does not suffer from this issue). Implement the
/// [`Handle<HostInvocation>`] directly to avoid the internal [`Clone::clone`].
pub struct HandlerFunc<F>(F);

impl<F> Deref for HandlerFunc<F> {
    type Target = F;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T, Fut, F> From<F> for HandlerFunc<F>
where
    T: Into<Vec<u8>>,
    Fut: Future<Output = Result<Option<T>>> + Sync + Send,
    F: Fn(jwt::Claims<jwt::Actor>, String, HostInvocation, Option<Vec<u8>>) -> Fut + Sync + Send,
{
    fn from(func: F) -> Self {
        Self(func)
    }
}

#[async_trait]
impl<T, Fut, F> Handle<HostInvocation> for HandlerFunc<F>
where
    T: Into<Vec<u8>>,
    Fut: Future<Output = Result<Option<T>>> + Sync + Send,
    F: Fn(jwt::Claims<jwt::Actor>, String, HostInvocation, Option<Vec<u8>>) -> Fut + Sync + Send,
{
    async fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        binding: String,
        invocation: HostInvocation,
        call_context: &Option<Vec<u8>>,
    ) -> Result<Option<Vec<u8>>> {
        match self(claims.clone(), binding, invocation, call_context.clone()).await {
            Ok(Some(res)) => Ok(Some(res.into())),
            Ok(None) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

/// A handler which wraps a synchronous function
pub struct HandlerFuncSync<F>(F);

impl<F> Deref for HandlerFuncSync<F> {
    type Target = F;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T, F> From<F> for HandlerFuncSync<F>
where
    T: Into<Vec<u8>>,
    F: Fn(&jwt::Claims<jwt::Actor>, String, HostInvocation) -> Result<Option<T>> + Sync + Send,
{
    fn from(func: F) -> Self {
        Self(func)
    }
}

#[async_trait]
impl<T, F> Handle<HostInvocation> for HandlerFuncSync<F>
where
    T: Into<Vec<u8>>,
    F: Fn(&jwt::Claims<jwt::Actor>, String, HostInvocation, &Option<Vec<u8>>) -> Result<Option<T>>
        + Sync
        + Send,
{
    async fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        binding: String,
        invocation: HostInvocation,
        call_context: &Option<Vec<u8>>,
    ) -> Result<Option<Vec<u8>>> {
        match self(claims, binding, invocation, call_context) {
            Ok(Some(res)) => Ok(Some(res.into())),
            Ok(None) => Ok(None),
            Err(err) => Err(err),
        }
    }
}
