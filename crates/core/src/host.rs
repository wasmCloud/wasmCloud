//! Reusable functionality related to [wasmCloud hosts][docs-wasmcloud-hosts]
//!
//! [docs-wasmcloud-hosts]: <https://wasmcloud.com/docs/concepts/hosts>

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::logging::Level;
use crate::otel::OtelConfig;

/// Extensions that provide extra functionality to the lattice in the form of binaries
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ExtensionData {
    #[serde(default)]
    pub lattice_rpc_prefix: String,
    #[serde(default)]
    pub lattice_rpc_user_jwt: String,
    #[serde(default)]
    pub lattice_rpc_user_seed: String,
    #[serde(default)]
    pub lattice_rpc_url: String,
    /// Named reference of the provider.
    #[serde(default)]
    pub provider_id: String,
    /// Unique uuid for this extension instance
    #[serde(default)]
    pub instance_id: String,
    /// default RPC timeout for rpc messages, in milliseconds.  Defaults to 2000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_rpc_timeout_ms: Option<u64>,
    #[serde(default)]
    pub structured_logging: bool,
    /// The log level providers should log at
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<Level>,
    #[serde(default)]
    pub otel_config: OtelConfig,
    /// The host id this extension is for. If internal, provided via stdin, if external, explicitly has to connect to a host.
    pub host_id: String,
}

impl ExtensionData {
    /// Load extension data from environment variables for external providers.
    pub fn from_env() -> Result<Self, String> {
        let lattice_rpc_prefix = std::env::var("WASMCLOUD_LATTICE")
            .map_err(|_| "WASMCLOUD_LATTICE environment variable is required")?;

        let provider_id = std::env::var("WASMCLOUD_PROVIDER_ID")
            .map_err(|_| "WASMCLOUD_PROVIDER_ID environment variable is required")?;

        let lattice_rpc_url = std::env::var("WASMCLOUD_NATS_URL")
            .unwrap_or_else(|_| "nats://127.0.0.1:4222".to_string());

        let lattice_rpc_user_jwt = std::env::var("WASMCLOUD_NATS_JWT").unwrap_or_default();

        let lattice_rpc_user_seed = std::env::var("WASMCLOUD_NATS_SEED").unwrap_or_default();

        let instance_id = Uuid::new_v4().to_string();

        let host_id = std::env::var("WASMCLOUD_HOST_ID").map_err(|_| {
            "WASMCLOUD_HOST_ID environment variable is required for external providers"
        })?;

        let default_rpc_timeout_ms = std::env::var("WASMCLOUD_RPC_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse().ok());

        let structured_logging = std::env::var("WASMCLOUD_STRUCTURED_LOGGING")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);

        let log_level = std::env::var("WASMCLOUD_LOG_LEVEL")
            .ok()
            .and_then(|v| v.parse().ok());

        // OTel configuration from environment
        // Supports both WASMCLOUD_OTEL_* and standard OTEL_* environment variables
        let otel_config = OtelConfig {
            enable_observability: std::env::var("WASMCLOUD_OTEL_ENABLE_OBSERVABILITY")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false),
            enable_traces: std::env::var("WASMCLOUD_OTEL_ENABLE_TRACES")
                .ok()
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1"),
            enable_metrics: std::env::var("WASMCLOUD_OTEL_ENABLE_METRICS")
                .ok()
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1"),
            enable_logs: std::env::var("WASMCLOUD_OTEL_ENABLE_LOGS")
                .ok()
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1"),
            observability_endpoint: std::env::var("WASMCLOUD_OTEL_ENDPOINT")
                .or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT"))
                .ok(),
            traces_endpoint: std::env::var("WASMCLOUD_OTEL_TRACES_ENDPOINT")
                .or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"))
                .ok(),
            metrics_endpoint: std::env::var("WASMCLOUD_OTEL_METRICS_ENDPOINT")
                .or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT"))
                .ok(),
            logs_endpoint: std::env::var("WASMCLOUD_OTEL_LOGS_ENDPOINT")
                .or_else(|_| std::env::var("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT"))
                .ok(),
            protocol: std::env::var("WASMCLOUD_OTEL_PROTOCOL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            additional_ca_paths: std::env::var("WASMCLOUD_OTEL_CA_PATHS")
                .ok()
                .map(|v| v.split(',').map(std::path::PathBuf::from).collect())
                .unwrap_or_default(),
            trace_level: std::env::var("WASMCLOUD_OTEL_TRACE_LEVEL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            traces_sampler: std::env::var("OTEL_TRACES_SAMPLER").ok(),
            traces_sampler_arg: std::env::var("OTEL_TRACES_SAMPLER_ARG").ok(),
            concurrent_exports: std::env::var("OTEL_BSP_MAX_CONCURRENT_EXPORTS")
                .ok()
                .and_then(|v| v.parse().ok()),
            max_batch_queue_size: std::env::var("OTEL_BSP_MAX_QUEUE_SIZE")
                .ok()
                .and_then(|v| v.parse().ok()),
        };

        Ok(Self {
            lattice_rpc_prefix,
            lattice_rpc_user_jwt,
            lattice_rpc_user_seed,
            lattice_rpc_url,
            provider_id,
            instance_id,
            default_rpc_timeout_ms,
            structured_logging,
            log_level,
            otel_config,
            host_id,
        })
    }
}
