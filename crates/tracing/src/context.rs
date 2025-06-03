//! Contains helpers and code for enabling [OpenTelemetry](https://opentelemetry.io/) tracing for
//! wasmcloud. Please note that right now this is only supported for providers. This module is only
//! available with the `otel` feature enabled

use std::collections::HashMap;
use std::ops::Deref;

use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::trace::TraceContextExt;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing::span::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use wasmcloud_core::TraceContext;

/// A convenience type that wraps an invocation [`TraceContext`] and implements the [`Extractor`] trait
#[derive(Debug)]
pub struct TraceContextExtractor<'a> {
    inner: &'a TraceContext,
}

impl<'a> TraceContextExtractor<'a> {
    /// Creates a new extractor using the given [`TraceContext`]
    #[must_use]
    pub fn new(context: &'a TraceContext) -> Self {
        TraceContextExtractor { inner: context }
    }
}

impl Extractor for TraceContextExtractor<'_> {
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

/// A convenience type that wraps an invocation [`TraceContext`] and implements the [`Injector`] trait
#[derive(Clone, Debug, Default)]
pub struct TraceContextInjector {
    inner: HashMap<String, String>,
}

impl TraceContextInjector {
    /// Creates a new injector using the given [`TraceContext`]
    #[must_use]
    pub fn new(headers: TraceContext) -> Self {
        // NOTE(thomastaylor312): Same point here with performance, technically we aren't allocating anything here except the hashmap, but we could do more optimization here if needed
        // Manually constructing the map here so we are sure we're only allocating once
        let mut inner = HashMap::with_capacity(headers.len());
        inner.extend(headers);
        TraceContextInjector { inner }
    }

    /// Convenience constructor that returns a new injector with the current span context already
    /// injected into the given header map
    #[must_use]
    pub fn new_with_span(headers: TraceContext) -> Self {
        let mut header_map = Self::new(headers);
        header_map.inject_context();
        header_map
    }

    // Creates a new injector with the context extracted from the given extractor. If the context is empty, it will use the current span's context
    pub fn new_with_extractor(extractor: &dyn Extractor) -> Self {
        let mut header_map = Self::default();
        let ctx_propagator = TraceContextPropagator::new();
        let context = ctx_propagator.extract(extractor);

        // Check if the extracted context is empty and use the current span's context if necessary
        if !context.span().span_context().is_valid() {
            ctx_propagator.inject_context(&Span::current().context(), &mut header_map);
        } else {
            ctx_propagator.inject_context(&context, &mut header_map);
        }

        header_map
    }

    /// Convenience constructor that returns a new injector with the current span context already
    /// injected into a default [`TraceContext`]
    #[must_use]
    pub fn default_with_span() -> Self {
        let mut header_map = Self::default();
        header_map.inject_context();
        header_map
    }

    /// Injects the context from the current span into the headers
    pub fn inject_context(&mut self) {
        let ctx_propagator = TraceContextPropagator::new();
        ctx_propagator.inject_context(&Span::current().context(), self);
    }

    /// Injects the context from the given span into the headers
    pub fn inject_context_from_span(&mut self, span: &Span) {
        let ctx_propagator = TraceContextPropagator::new();
        ctx_propagator.inject_context(&span.context(), self);
    }
}

impl Injector for TraceContextInjector {
    fn set(&mut self, key: &str, value: String) {
        self.inner.insert(key.to_owned(), value);
    }
}

impl AsRef<HashMap<String, String>> for TraceContextInjector {
    fn as_ref(&self) -> &HashMap<String, String> {
        &self.inner
    }
}

impl Deref for TraceContextInjector {
    type Target = HashMap<String, String>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<TraceContext> for TraceContextInjector {
    fn from(context: TraceContext) -> Self {
        TraceContextInjector::new(context)
    }
}

impl From<TraceContextInjector> for TraceContext {
    fn from(inj: TraceContextInjector) -> Self {
        inj.inner.into_iter().collect()
    }
}

/// A convenience function that will extract the [`opentelemetry::Context`] from the given
/// [`TraceContext`]. If you want to do something more advanced, use the [`TraceContextExtractor`]
pub fn get_span_context(trace_context: &TraceContext) -> opentelemetry::Context {
    let ctx_propagator = TraceContextPropagator::new();
    let extractor = TraceContextExtractor::new(trace_context);
    ctx_propagator.extract(&extractor)
}

/// A convenience function that will extract from an incoming context and set the parent span for
/// the current tracing Span. If you want to do something more advanced, use the
/// [`TraceContextExtractor`] type directly
///
/// **WARNING**: To avoid performance issues, this function does not check if you have empty tracing
/// headers. **If you pass an empty Extractor to this function, you will orphan the current span
/// hierarchy.**
#[allow(clippy::module_name_repetitions)]
pub fn attach_span_context(trace_context: &TraceContext) {
    let parent_ctx = get_span_context(trace_context);
    Span::current().set_parent(parent_ctx);
}
