//! Core reusable logic around [OpenTelemetry ("OTEL")](https://opentelemetry.io/) support

use std::str::FromStr;

use anyhow::bail;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::wit::WitMap;

/// Configuration values for OpenTelemetry
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OtelConfig {
    /// Determine whether observability should be enabled.
    pub enable_observability: bool,
    /// Determine whether traces should be enabled.
    pub enable_traces: Option<bool>,
    /// Determine whether metrics should be enabled.
    pub enable_metrics: Option<bool>,
    /// Determine whether logs should be enabled.
    pub enable_logs: Option<bool>,
    /// Overrides the OpenTelemetry endpoint for all signals.
    pub observability_endpoint: Option<String>,
    /// Overrides the OpenTelemetry endpoint for traces.
    pub traces_endpoint: Option<String>,
    /// Overrides the OpenTelemetry endpoint for metrics.
    pub metrics_endpoint: Option<String>,
    /// Overrides the OpenTelemetry endpoint for logs.
    pub logs_endpoint: Option<String>,
    /// Determines whether http or grpc will be used for exporting the telemetry.
    #[serde(default)]
    pub protocol: OtelProtocol,
}

impl OtelConfig {
    pub fn logs_endpoint(&self) -> String {
        self.resolve_endpoint(OtelSignal::Logs, self.logs_endpoint.clone())
    }

    pub fn metrics_endpoint(&self) -> String {
        self.resolve_endpoint(OtelSignal::Metrics, self.metrics_endpoint.clone())
    }

    pub fn traces_endpoint(&self) -> String {
        self.resolve_endpoint(OtelSignal::Traces, self.traces_endpoint.clone())
    }

    pub fn logs_enabled(&self) -> bool {
        self.enable_logs.unwrap_or(self.enable_observability)
    }

    pub fn metrics_enabled(&self) -> bool {
        self.enable_metrics.unwrap_or(self.enable_observability)
    }

    pub fn traces_enabled(&self) -> bool {
        self.enable_traces.unwrap_or(self.enable_observability)
    }

    // We have 3 potential outcomes depending on the provided configuration:
    // 1. We are given a signal-specific endpoint to use, which we'll use as-is.
    // 2. We are given an endpoint that each of the signal paths should added to
    // 3. We are given nothing, and we should simply default to an empty string,
    //    which lets the opentelemetry-otlp library handle defaults appropriately.
    fn resolve_endpoint(
        &self,
        signal: OtelSignal,
        signal_endpoint_override: Option<String>,
    ) -> String {
        // If we have a signal specific endpoint override, use it as provided.
        if let Some(endpoint) = signal_endpoint_override {
            return endpoint;
        }

        if let Some(endpoint) = self.observability_endpoint.clone() {
            return match self.protocol {
                OtelProtocol::Grpc => self.resolve_grpc_endpoint(endpoint),
                OtelProtocol::Http => self.resolve_http_endpoint(signal, endpoint),
            };
        }

        // If we have no match, fall back to empty string to let the opentelemetry-otlp
        // library handling turn into the signal-specific default endpoint.
        String::new()
    }

    // opentelemetry-otlp expects the gRPC endpoint to not have path components
    // configured, so we're just clearing them out and returning the base url.
    fn resolve_grpc_endpoint(&self, endpoint: String) -> String {
        match Url::parse(&endpoint) {
            Ok(mut url) => {
                if let Ok(mut path) = url.path_segments_mut() {
                    path.clear();
                }
                url.as_str().trim_end_matches('/').to_string()
            }
            Err(_) => endpoint,
        }
    }

    // opentelemetry-otlp expects the http endpoint to be fully configured
    // including the path, so we check whether there's a path already configured
    // and use the url as configured, or append the signal-specific path to the
    // provided endpoint.
    fn resolve_http_endpoint(&self, signal: OtelSignal, endpoint: String) -> String {
        match Url::parse(&endpoint) {
            Ok(url) => {
                if url.path() == "/" {
                    format!("{}{}", url.as_str().trim_end_matches('/'), signal)
                } else {
                    endpoint
                }
            }
            Err(_) => endpoint,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum OtelProtocol {
    Grpc,
    Http,
}

// Represents https://opentelemetry.io/docs/concepts/signals/
enum OtelSignal {
    Traces,
    Metrics,
    Logs,
}

impl std::fmt::Display for OtelSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "/v1/{}",
            match self {
                OtelSignal::Traces => "traces",
                OtelSignal::Metrics => "metrics",
                OtelSignal::Logs => "logs",
            }
        )
    }
}

impl Default for OtelProtocol {
    fn default() -> Self {
        Self::Http
    }
}

impl FromStr for OtelProtocol {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "http" => Ok(Self::Http),
            "grpc" => Ok(Self::Grpc),
            protocol => {
                bail!("unsupported protocol: {protocol:?}, did you mean 'http' or 'grpc'?")
            }
        }
    }
}

/// Environment settings for initializing a capability provider
pub type TraceContext = WitMap<String>;

#[cfg(test)]
mod tests {
    use super::{OtelConfig, OtelProtocol};

    #[test]
    fn test_grpc_resolves_to_empty_string_without_overrides() {
        let config = OtelConfig {
            protocol: OtelProtocol::Grpc,
            ..Default::default()
        };

        let expected = String::from("");

        assert_eq!(expected, config.traces_endpoint());
        assert_eq!(expected, config.metrics_endpoint());
        assert_eq!(expected, config.logs_endpoint());
    }

    #[test]
    fn test_grpc_resolves_to_base_url_without_path_components() {
        let config = OtelConfig {
            protocol: OtelProtocol::Grpc,
            observability_endpoint: Some(String::from(
                "https://example.com:4318/path/does/not/exist",
            )),
            ..Default::default()
        };

        let expected = String::from("https://example.com:4318");

        assert_eq!(expected, config.traces_endpoint());
        assert_eq!(expected, config.metrics_endpoint());
        assert_eq!(expected, config.logs_endpoint());
    }

    #[test]
    fn test_grpc_resolves_to_signal_specific_overrides_as_provided() {
        let config = OtelConfig {
            protocol: OtelProtocol::Grpc,
            traces_endpoint: Some(String::from("https://example.com:4318/path/does/not/exist")),
            ..Default::default()
        };

        let expected_traces = String::from("https://example.com:4318/path/does/not/exist");
        let expected_others = String::from("");

        assert_eq!(expected_traces, config.traces_endpoint());
        assert_eq!(expected_others, config.metrics_endpoint());
        assert_eq!(expected_others, config.logs_endpoint());
    }

    #[test]
    fn test_http_resolves_to_empty_string_without_overrides() {
        let config = OtelConfig {
            protocol: OtelProtocol::Http,
            ..Default::default()
        };

        let expected = String::from("");

        assert_eq!(expected, config.traces_endpoint());
        assert_eq!(expected, config.metrics_endpoint());
        assert_eq!(expected, config.logs_endpoint());
    }

    #[test]
    fn test_http_configuration_for_specific_signal_should_not_affect_other_signals() {
        let config = OtelConfig {
            protocol: OtelProtocol::Http,
            traces_endpoint: Some(String::from(
                "https://example.com:4318/v1/traces/or/something",
            )),
            ..Default::default()
        };

        let expected_traces = String::from("https://example.com:4318/v1/traces/or/something");
        let expected_others = String::from("");

        assert_eq!(expected_traces, config.traces_endpoint());
        assert_eq!(expected_others, config.metrics_endpoint());
        assert_eq!(expected_others, config.logs_endpoint());
    }

    #[test]
    fn test_http_should_be_configurable_across_all_signals_via_observability_endpoint() {
        let config = OtelConfig {
            protocol: OtelProtocol::Http,
            observability_endpoint: Some(String::from("https://example.com:4318")),
            ..Default::default()
        };

        let expected_traces = String::from("https://example.com:4318/v1/traces");
        let expected_metrics = String::from("https://example.com:4318/v1/metrics");
        let expected_logs = String::from("https://example.com:4318/v1/logs");

        assert_eq!(expected_traces, config.traces_endpoint());
        assert_eq!(expected_metrics, config.metrics_endpoint());
        assert_eq!(expected_logs, config.logs_endpoint());
    }
}
