use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing::span::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;

/// Helper for injecting headers into HTTP Requests. This is used for OpenTelemetry context
/// propagation over HTTP.
/// See [this](https://github.com/open-telemetry/opentelemetry-rust/blob/main/examples/tracing-http-propagator/README.md)
/// for example usage.
pub struct HeaderInjector<'a>(pub &'a mut http::HeaderMap);

impl Injector for HeaderInjector<'_> {
    /// Set a key and value in the HeaderMap.  Does nothing if the key or value are not valid inputs.
    fn set(&mut self, key: &str, value: String) {
        if let Ok(name) = http::header::HeaderName::from_bytes(key.as_bytes()) {
            if let Ok(val) = http::header::HeaderValue::from_str(&value) {
                self.0.insert(name, val);
            }
        }
    }
}

impl HeaderInjector<'_> {
    /// Injects the context from the current span into the headers
    pub fn inject_context(&mut self) {
        let ctx_propagator = TraceContextPropagator::new();
        ctx_propagator.inject_context(&Span::current().context(), self);
    }
}

/// Helper for extracting headers from HTTP Requests. This is used for OpenTelemetry context
/// propagation over HTTP.
/// See [this](https://github.com/open-telemetry/opentelemetry-rust/blob/main/examples/tracing-http-propagator/README.md)
/// for example usage.
pub struct HeaderExtractor<'a>(pub &'a http::HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    /// Get a value for a key from the HeaderMap.  If the value is not valid ASCII, returns None.
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    /// Collect all the keys from the HeaderMap.
    fn keys(&self) -> Vec<&str> {
        self.0
            .keys()
            .map(|value| value.as_str())
            .collect::<Vec<_>>()
    }
}
