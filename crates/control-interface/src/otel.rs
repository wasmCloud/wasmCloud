// COPIED DIRECTLY FROM https://github.com/wasmCloud/weld/blob/wasmbus-rpc-v0.13.0/rpc-rs/src/otel.rs (minus unused functionality)

//! Contains helpers and code for enabling [OpenTelemetry](https://opentelemetry.io/) tracing for
//! wasmbus-rpc calls. Please note that right now this is only supported for providers. This module
//! is only available with the `otel` feature enabled

use async_nats::header::HeaderMap;
use opentelemetry::propagation::{Injector, TextMapPropagator};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing::span::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// A convenience type that wraps a NATS [`HeaderMap`] and implements the [`Injector`] trait
#[derive(Debug, Default)]
pub struct HeaderInjector {
    inner: HeaderMap,
}

impl HeaderInjector {
    /// Creates a new injector using the given [`HeaderMap`]
    pub fn new(headers: HeaderMap) -> Self {
        HeaderInjector { inner: headers }
    }

    /// Convenience constructor that returns a new injector with the current span context already
    /// injected into the given header map
    pub fn new_with_span(headers: HeaderMap) -> Self {
        let mut header_map = Self::new(headers);
        header_map.inject_context();
        header_map
    }

    /// Convenience constructor that returns a new injector with the current span context already
    /// injected into a default [`HeaderMap`]
    pub fn default_with_span() -> Self {
        let mut header_map = Self::default();
        header_map.inject_context();
        header_map
    }

    /// Injects the current context from the span into the headers
    pub fn inject_context(&mut self) {
        let ctx_propagator = TraceContextPropagator::new();
        ctx_propagator.inject_context(&Span::current().context(), self);
    }
}

impl Injector for HeaderInjector {
    fn set(&mut self, key: &str, value: String) {
        self.inner.insert(key, value.as_ref());
    }
}

impl AsRef<HeaderMap> for HeaderInjector {
    fn as_ref(&self) -> &HeaderMap {
        &self.inner
    }
}

impl From<HeaderMap> for HeaderInjector {
    fn from(headers: HeaderMap) -> Self {
        HeaderInjector::new(headers)
    }
}

impl From<HeaderInjector> for HeaderMap {
    fn from(inj: HeaderInjector) -> Self {
        inj.inner
    }
}
