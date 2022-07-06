//! Contains helpers and code for enabling [OpenTelemetry](https://opentelemetry.io/) tracing for
//! wasmbus-rpc calls. Please note that right now this is only supported for providers. This module
//! is only available with the `otel` feature enabled

use async_nats::header::HeaderMap;
use opentelemetry::{
    propagation::{Extractor, Injector, TextMapPropagator},
    sdk::propagation::TraceContextPropagator,
};
use tracing::span::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

lazy_static::lazy_static! {
    static ref EMPTY_HEADERS: HeaderMap = HeaderMap::default();
}

/// A convenience type that wraps a NATS [`HeaderMap`] and implements the [`Extractor`] trait
#[derive(Debug)]
pub struct OtelHeaderExtractor<'a> {
    inner: &'a HeaderMap,
}

impl<'a> OtelHeaderExtractor<'a> {
    /// Creates a new extractor using the given [`HeaderMap`]
    pub fn new(headers: &'a HeaderMap) -> Self {
        OtelHeaderExtractor { inner: headers }
    }

    /// Creates a new extractor using the given message
    pub fn new_from_message(msg: &'a async_nats::Message) -> Self {
        OtelHeaderExtractor {
            inner: msg.headers.as_ref().unwrap_or(&EMPTY_HEADERS),
        }
    }
}

impl<'a> Extractor for OtelHeaderExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.inner.get(key).and_then(|s| s.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.inner.keys().map(|s| s.as_str()).collect()
    }
}

impl<'a> AsRef<HeaderMap> for OtelHeaderExtractor<'a> {
    fn as_ref(&self) -> &'a HeaderMap {
        self.inner
    }
}

/// A convenience type that wraps a NATS [`HeaderMap`] and implements the [`Injector`] trait
#[derive(Debug, Default)]
pub struct OtelHeaderInjector {
    inner: HeaderMap,
}

impl OtelHeaderInjector {
    /// Creates a new injector using the given [`HeaderMap`]
    pub fn new(headers: HeaderMap) -> Self {
        OtelHeaderInjector { inner: headers }
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

impl Injector for OtelHeaderInjector {
    fn set(&mut self, key: &str, value: String) {
        // NOTE: Because the underlying headers are an http header, we are going to escape any
        // unicode values and non-printable ASCII chars, which sounds better than just silently
        // ignoring or using an empty string. Unfortunately this adds an extra allocation that is
        // probably ok for now as it is freed at the end, but I prefer telemetry stuff to be as
        // little overhead as possible. If anyone has a better idea of how to handle this, please PR
        // it in
        let header_name = key.escape_default().to_string().into_bytes();
        let escaped = value.escape_default().to_string().into_bytes();
        // SAFETY: All chars escaped above
        self.inner.insert(
            async_nats::header::HeaderName::from_bytes(&header_name).unwrap(),
            async_nats::HeaderValue::from_bytes(&escaped).unwrap(),
        );
    }
}

impl AsRef<HeaderMap> for OtelHeaderInjector {
    fn as_ref(&self) -> &HeaderMap {
        &self.inner
    }
}

impl From<HeaderMap> for OtelHeaderInjector {
    fn from(headers: HeaderMap) -> Self {
        OtelHeaderInjector::new(headers)
    }
}

impl From<OtelHeaderInjector> for HeaderMap {
    fn from(inj: OtelHeaderInjector) -> Self {
        inj.inner
    }
}

/// A convenience function that will extract the current context from NATS message headers and set
/// the parent span for the current tracing Span. If you want to do something more advanced, use the
/// [`OtelHeaderExtractor`] type directly
pub fn attach_span_context(msg: &async_nats::Message) {
    let header_map = OtelHeaderExtractor::new_from_message(msg);
    let ctx_propagator = TraceContextPropagator::new();
    let parent_ctx = ctx_propagator.extract(&header_map);
    Span::current().set_parent(parent_ctx);
}
