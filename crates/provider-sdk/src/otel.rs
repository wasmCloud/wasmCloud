//! Contains helpers and code for enabling [OpenTelemetry](https://opentelemetry.io/) tracing for
//! wasmbus-rpc calls. Please note that right now this is only supported for providers. This module
//! is only available with the `otel` feature enabled

use std::collections::HashMap;

use opentelemetry::{
    propagation::{Extractor, Injector, TextMapPropagator},
    sdk::propagation::TraceContextPropagator,
};
use tracing::span::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::core::{Invocation, TraceContext};

/// A convenience type that wraps an invocation [`TraceContext`] and implements the [`Extractor`] trait
#[derive(Debug)]
pub struct OtelHeaderExtractor<'a> {
    inner: &'a TraceContext,
}

impl<'a> OtelHeaderExtractor<'a> {
    /// Creates a new extractor using the given [`HeaderMap`]
    pub fn new(headers: &'a TraceContext) -> Self {
        OtelHeaderExtractor { inner: headers }
    }

    /// Creates a new extractor using the given invocation
    pub fn new_from_message(inv: &'a Invocation) -> Self {
        let inner = inv.trace_context.as_ref();
        OtelHeaderExtractor { inner }
    }
}

impl<'a> Extractor for OtelHeaderExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        // NOTE(thomastaylor312): I don't like that we have to iterate to find this, but I didn't
        // want to allocate hashmap for now. If this starts to cause performance issues, we can see
        // what the tradeoff is for increasing space usage for a faster lookup
        self.inner
            .iter()
            .find_map(|(k, v)| (k == key).then_some(v.as_str()))
    }

    fn keys(&self) -> Vec<&str> {
        self.inner.iter().map(|(k, _)| k.as_str()).collect()
    }
}

impl<'a> AsRef<TraceContext> for OtelHeaderExtractor<'a> {
    fn as_ref(&self) -> &'a TraceContext {
        self.inner
    }
}

/// A convenience type that wraps an invocation [`TraceContext`] and implements the [`Injector`] trait
#[derive(Debug, Default)]
pub struct OtelHeaderInjector {
    inner: HashMap<String, String>,
}

impl OtelHeaderInjector {
    /// Creates a new injector using the given [`TraceContext`]
    pub fn new(headers: TraceContext) -> Self {
        // NOTE(thomastaylor312): Same point here with performance, technically we aren't allocating anything here except the hashmap, but we could do more optimization here if needed
        // Manually constructing the map here so we are sure we're only allocating once
        let mut inner = HashMap::with_capacity(headers.len());
        inner.extend(headers.into_iter());
        OtelHeaderInjector { inner }
    }

    /// Convenience constructor that returns a new injector with the current span context already
    /// injected into the given header map
    pub fn new_with_span(headers: TraceContext) -> Self {
        let mut header_map = Self::new(headers);
        header_map.inject_context();
        header_map
    }

    /// Convenience constructor that returns a new injector with the current span context already
    /// injected into a default [`TraceContext`]
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
        self.inner.insert(key.to_owned(), value);
    }
}

impl From<TraceContext> for OtelHeaderInjector {
    fn from(headers: TraceContext) -> Self {
        OtelHeaderInjector::new(headers)
    }
}

impl From<OtelHeaderInjector> for TraceContext {
    fn from(inj: OtelHeaderInjector) -> Self {
        inj.inner.into_iter().collect()
    }
}

/// A convenience function that will extract the current context from NATS message headers and set
/// the parent span for the current tracing Span. If you want to do something more advanced, use the
/// [`OtelHeaderExtractor`] type directly
pub fn attach_span_context(inv: &Invocation) {
    let header_map = OtelHeaderExtractor::new_from_message(inv);
    let ctx_propagator = TraceContextPropagator::new();
    let parent_ctx = ctx_propagator.extract(&header_map);
    Span::current().set_parent(parent_ctx);
}
