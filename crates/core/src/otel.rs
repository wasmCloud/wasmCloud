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
    /// Overrides the OpenTelemetry endpoint for all signals.
    pub observability_endpoint: Option<String>,
    /// Overrides the OpenTelemetry endpoint for tracing.
    pub tracing_endpoint: Option<String>,
    /// Overrides the OpenTelemetry endpoint for metrics.
    pub metrics_endpoint: Option<String>,
    /// Overrides the OpenTelemetry endpoint for logs.
    pub logs_endpoint: Option<String>,
}

/// Environment settings for initializing a capability provider
pub type TraceContext = WitMap<String>;
