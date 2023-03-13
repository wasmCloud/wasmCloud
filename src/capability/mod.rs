/// Builtin logging capabilities available within `wasmcloud:builtin:logging` namespace
pub mod logging;
/// Builtin random number generation capabilities available within `wasmcloud:builtin:numbergen` namespace
pub mod numbergen;

pub use logging::*;
pub use numbergen::*;

use core::fmt::Debug;
use std::sync::Arc;

use anyhow::{bail, Context};
use tracing::{instrument, trace_span};
use wascap::jwt;
use wasmbus_rpc::common::{deserialize, serialize};
use wasmcloud_interface_logging::LogEntry;
use wasmcloud_interface_numbergen::RangeLimit;

/// Capability handler
pub trait Handler {
    /// Error returned by [`Handler::handle`] operations
    type Error: ToString + Debug;

    /// Handles a raw capability provider invocation.
    ///
    /// # Errors
    ///
    /// Returns an [`anyhow::Error`] in case an error is non-recoverable, for example if an invalid
    /// payload is passed to a builtin provider, which will cause an exception in the guest.
    /// Innermost result represents the underlying operation result, which will be passed to the
    /// guest as an application-layer error.
    fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        binding: String,
        namespace: String,
        operation: String,
        payload: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, Self::Error>>;
}

/// A [Handler], which handles all builtin capability invocations using [Logging], [Numbergen] and
/// offloads all external capabilities to an arbitrary [Handler]
pub struct HostHandler<L, N, H> {
    /// Logging capability provider, using which all known `wasmcloud:builtin:logging` operations will be handled
    pub logging: L,

    /// Random number generator capability provider, using which all known `wasmcloud:builtin:numbergen` operations will be handled
    pub numbergen: N,

    /// External capability provider, using which all non-builtin calls will be handled
    pub external: H,
}

/// A builder for [`HostHandler`]
pub struct HostHandlerBuilder<L, N, H> {
    /// Logging capability provider, using which all known `wasmcloud:builtin:logging` operations will be handled
    pub logging: L,

    /// Random number generator capability provider, using which all known `wasmcloud:builtin:numbergen` operations will be handled
    pub numbergen: N,

    /// External capability provider, using which all non-builtin calls will be handled
    pub external: H,
}

#[cfg(all(feature = "rand", feature = "log"))]
impl<H>
    HostHandlerBuilder<LogLogging<&'static dyn ::log::Log>, RandNumbergen<::rand::rngs::OsRng>, H>
{
    /// Creates a new [`HostHandler`] builder with preset defaults
    pub fn new(hostcall: H) -> Self {
        Self {
            logging: LogLogging::from(::log::logger()),
            numbergen: RandNumbergen::from(::rand::rngs::OsRng),
            external: hostcall,
        }
    }
}

impl<L, N, H> From<HostHandlerBuilder<L, N, H>> for HostHandler<L, N, H> {
    fn from(builder: HostHandlerBuilder<L, N, H>) -> Self {
        builder.build()
    }
}

impl<L, N, H> From<HostHandlerBuilder<L, N, H>> for Arc<HostHandler<L, N, H>> {
    fn from(builder: HostHandlerBuilder<L, N, H>) -> Self {
        builder.build().into()
    }
}

impl<L, N, H> HostHandlerBuilder<L, N, H> {
    /// Set [Logging] handler
    pub fn logging<T: Logging>(self, logging: T) -> HostHandlerBuilder<T, N, H> {
        HostHandlerBuilder {
            logging,
            numbergen: self.numbergen,
            external: self.external,
        }
    }

    /// Set [Numbergen] handler
    pub fn numbergen<T: Numbergen>(self, numbergen: T) -> HostHandlerBuilder<L, T, H> {
        HostHandlerBuilder {
            numbergen,
            logging: self.logging,
            external: self.external,
        }
    }

    /// Set host call [Handler]
    pub fn hostcall<T: Handler>(self, hostcall: T) -> HostHandlerBuilder<L, N, T> {
        HostHandlerBuilder {
            external: hostcall,
            numbergen: self.numbergen,
            logging: self.logging,
        }
    }

    /// Turns this builder into a [`HostHandler`]
    pub fn build(self) -> HostHandler<L, N, H> {
        HostHandler {
            logging: self.logging,
            numbergen: self.numbergen,
            external: self.external,
        }
    }
}

impl Handler for () {
    type Error = &'static str;

    fn handle(
        &self,
        _: &jwt::Claims<jwt::Actor>,
        _: String,
        _: String,
        _: String,
        _: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, Self::Error>> {
        Ok(Err("not supported"))
    }
}

impl<T, E, F> Handler for F
where
    T: Into<Vec<u8>>,
    E: ToString + Debug,
    F: Fn(
        &jwt::Claims<jwt::Actor>,
        String,
        String,
        String,
        Vec<u8>,
    ) -> anyhow::Result<Result<T, E>>,
{
    type Error = E;

    fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        binding: String,
        namespace: String,
        operation: String,
        payload: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, Self::Error>> {
        match self(claims, binding, namespace, operation, payload) {
            Ok(Ok(res)) => Ok(Ok(res.into())),
            Ok(Err(err)) => Ok(Err(err)),
            Err(err) => Err(err),
        }
    }
}

impl<L, N, H> Handler for HostHandler<L, N, H>
where
    L: Logging,
    N: Numbergen,
    H: Handler,
{
    type Error = String;

    #[instrument(skip(self))]
    fn handle(
        &self,
        claims: &jwt::Claims<jwt::Actor>,
        binding: String,
        namespace: String,
        operation: String,
        payload: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, Self::Error>> {
        match (binding.as_str(), namespace.as_str(), operation.as_str()) {
            (_, "wasmcloud:builtin:logging", "Logging.WriteLog") => {
                let LogEntry { level, text } =
                    deserialize(&payload).context("failed to deserialize log entry")?;
                let res =
                    match level.as_str() {
                        "debug" => trace_span!("Logging::debug")
                            .in_scope(|| self.logging.debug(claims, text)),
                        "info" => trace_span!("Logging::info")
                            .in_scope(|| self.logging.info(claims, text)),
                        "warn" => trace_span!("Logging::warn")
                            .in_scope(|| self.logging.warn(claims, text)),
                        "error" => trace_span!("Logging::error")
                            .in_scope(|| self.logging.error(claims, text)),
                        _ => {
                            bail!("log level `{level}` is not supported")
                        }
                    };
                match res {
                    Ok(()) => Ok(Ok(vec![])),
                    Err(err) => Ok(Err(err.to_string())),
                }
            }
            (_, "wasmcloud:builtin:numbergen", "NumberGen.GenerateGuid") => {
                match trace_span!("Numbergen::generate_guid")
                    .in_scope(|| self.numbergen.generate_guid(claims))
                {
                    Ok(guid) => serialize(&guid.to_string())
                        .context("failed to serialize UUID")
                        .map(Ok),
                    Err(err) => Ok(Err(err.to_string())),
                }
            }
            (_, "wasmcloud:builtin:numbergen", "NumberGen.RandomInRange") => {
                let RangeLimit { min, max } =
                    deserialize(&payload).context("failed to deserialize range limit")?;
                match trace_span!("Numbergen::random_in_range")
                    .in_scope(|| self.numbergen.random_in_range(claims, min, max))
                {
                    Ok(v) => serialize(&v).context("failed to serialize number").map(Ok),
                    Err(err) => Ok(Err(err.to_string())),
                }
            }
            (_, "wasmcloud:builtin:numbergen", "NumberGen.Random32") => {
                match trace_span!("Numbergen::random_32")
                    .in_scope(|| self.numbergen.random_32(claims))
                {
                    Ok(v) => serialize(&v).context("failed to serialize number").map(Ok),
                    Err(err) => Ok(Err(err.to_string())),
                }
            }
            _ => match trace_span!("Handler::handle").in_scope(|| {
                self.external
                    .handle(claims, binding, namespace, operation, payload)
            }) {
                Ok(Ok(res)) => Ok(Ok(res)),
                Ok(Err(err)) => Ok(Err(err.to_string())),
                Err(err) => Err(err),
            },
        }
    }
}
