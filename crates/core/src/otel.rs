//! Core reusable logic around [OpenTelemetry ("OTEL")](https://opentelemetry.io/) support

use serde::{Deserialize, Serialize};

use crate::wit::WitMap;

/// Configuration values for Open Telemetry
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OtelConfig {
    /// Determine whether observability should be enabled.
    pub enable_observability: bool,
    /// Determine whether tracing should be enabled.
    pub enable_tracing: Option<bool>,
    /// Determine whether metrics should be enabled.
    pub enable_metrics: Option<bool>,
    /// Determine whether logs should be enabled.
    pub enable_logs: Option<bool>,
    /// OTEL_TRACES_EXPORTER https://opentelemetry.io/docs/concepts/sdk-configuration/general-sdk-configuration/#otel_traces_exporter
    pub traces_exporter: Option<String>,
    /// OTEL_EXPORTER_OTLP_ENDPOINT https://opentelemetry.io/docs/concepts/sdk-configuration/otlp-exporter-configuration/#otel_exporter_otlp_endpoint
    pub exporter_otlp_endpoint: Option<String>,
}

/// Environment settings for initializing a capability provider
pub type TraceContext = WitMap<String>;
