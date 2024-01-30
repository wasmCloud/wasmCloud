use std::sync::OnceLock;

use async_nats::header::{HeaderMap, HeaderValue};
use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing::span::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

static EMPTY_HEADERS: OnceLock<HeaderMap> = OnceLock::new();

fn empty_headers() -> &'static HeaderMap {
    EMPTY_HEADERS.get_or_init(HeaderMap::new)
}

/// A convenience type that wraps a NATS [`HeaderMap`] and implements the [`Extractor`] trait
#[derive(Debug)]
pub struct NatsHeaderExtractor<'a> {
    inner: &'a HeaderMap,
}

impl<'a> NatsHeaderExtractor<'a> {
    /// Creates a new extractor using the given [`HeaderMap`]
    pub fn new(headers: &'a HeaderMap) -> Self {
        NatsHeaderExtractor { inner: headers }
    }

    /// Creates a new extractor using the given message
    pub fn new_from_message(msg: &'a async_nats::Message) -> Self {
        let inner = msg.headers.as_ref().unwrap_or_else(|| empty_headers());
        NatsHeaderExtractor { inner }
    }
}

impl<'a> Extractor for NatsHeaderExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.inner.get(key).map(HeaderValue::as_str)
    }

    fn keys(&self) -> Vec<&str> {
        self.inner
            .iter()
            // The underlying type is a string and this should never fail, but we unwrap to an empty string anyway
            .map(|(k, _)| std::str::from_utf8(k.as_ref()).unwrap_or_default())
            .collect()
    }
}

impl<'a> AsRef<HeaderMap> for NatsHeaderExtractor<'a> {
    fn as_ref(&self) -> &'a HeaderMap {
        self.inner
    }
}

/// A convenience type that wraps a NATS [`HeaderMap`] and implements the [`Injector`] trait
#[derive(Debug, Default)]
pub struct NatsHeaderInjector {
    inner: HeaderMap,
}

impl NatsHeaderInjector {
    /// Creates a new injector using the given [`HeaderMap`]
    pub fn new(headers: HeaderMap) -> Self {
        NatsHeaderInjector { inner: headers }
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

impl Injector for NatsHeaderInjector {
    fn set(&mut self, key: &str, value: String) {
        self.inner.insert(key, value.as_ref());
    }
}

impl AsRef<HeaderMap> for NatsHeaderInjector {
    fn as_ref(&self) -> &HeaderMap {
        &self.inner
    }
}

impl From<HeaderMap> for NatsHeaderInjector {
    fn from(headers: HeaderMap) -> Self {
        NatsHeaderInjector::new(headers)
    }
}

impl From<NatsHeaderInjector> for HeaderMap {
    fn from(inj: NatsHeaderInjector) -> Self {
        inj.inner
    }
}

/// A convenience function that will extract headers from a message and set the parent span for the
/// current tracing Span.  If you want to do something more advanced, use the
/// [`NatsHeaderExtractor`] type directly
pub fn attach_span_context(msg: &async_nats::Message) {
    // If we extract and there are no OTEL headers, setting the parent will orphan the current span
    // hierarchy. Checking that there are headers is a heuristic to avoid this
    if let Some(ref headers) = msg.headers {
        if headers.iter().len() > 0 {
            let extractor = NatsHeaderExtractor::new(headers);
            let ctx_propagator = TraceContextPropagator::new();
            let parent_ctx = ctx_propagator.extract(&extractor);
            Span::current().set_parent(parent_ctx);
        }
    }
}
