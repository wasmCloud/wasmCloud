//! Core reusable logic around [OpenTelemetry ("OTEL")](https://opentelemetry.io/) support

use std::str::FromStr;

use anyhow::bail;
use serde::{Deserialize, Serialize};

use crate::wit::WitMap;

// We redefine the upstream variables here since they are not exported by the opentelemetry-otlp crate:
// https://github.com/open-telemetry/opentelemetry-rust/blob/opentelemetry-0.23.0/opentelemetry-otlp/src/exporter/mod.rs#L57-L60
const OTEL_EXPORTER_OTLP_GRPC_ENDPOINT_DEFAULT: &str = "http://localhost:4317";
const OTEL_EXPORTER_OTLP_HTTP_ENDPOINT_DEFAULT: &str = "http://localhost:4318";

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
        self.logs_endpoint.clone().unwrap_or_else(|| {
            self.observability_endpoint
                .clone()
                .unwrap_or_else(|| self.default_endpoint().to_string())
        })
    }

    pub fn metrics_endpoint(&self) -> String {
        self.metrics_endpoint.clone().unwrap_or_else(|| {
            self.observability_endpoint
                .clone()
                .unwrap_or_else(|| self.default_endpoint().to_string())
        })
    }

    pub fn traces_endpoint(&self) -> String {
        self.traces_endpoint.clone().unwrap_or_else(|| {
            self.observability_endpoint
                .clone()
                .unwrap_or_else(|| self.default_endpoint().to_string())
        })
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

    fn default_endpoint(&self) -> &str {
        match self.protocol {
            OtelProtocol::Grpc => OTEL_EXPORTER_OTLP_GRPC_ENDPOINT_DEFAULT,
            OtelProtocol::Http => OTEL_EXPORTER_OTLP_HTTP_ENDPOINT_DEFAULT,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum OtelProtocol {
    Grpc,
    Http,
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
