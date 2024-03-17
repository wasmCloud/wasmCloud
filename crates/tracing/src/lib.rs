#[cfg(feature = "otel")]
use heck::ToKebabCase;
#[cfg(feature = "otel")]
pub use opentelemetry::{
    global,
    metrics::{Counter, Histogram, Meter, Unit},
    KeyValue,
};
use wasmcloud_core::logging::Level;
use wasmcloud_core::OtelConfig;

#[cfg(feature = "otel")]
pub mod context;

mod traces;

mod metrics;

#[cfg(not(feature = "otel"))]
pub fn configure_observability(
    _: &str,
    _: &OtelConfig,
    use_structured_logging: bool,
    log_level_override: Option<&Level>,
) -> anyhow::Result<()> {
    // if OTEL is not enabled, explicitly do not emit observability
    let otel_config = OtelConfig::default();
    traces::configure_tracing("", &otel_config, use_structured_logging, log_level_override)
}

/// Configures observability for each type of signal
#[cfg(feature = "otel")]
pub fn configure_observability(
    service_name: &str,
    otel_config: &OtelConfig,
    use_structured_logging: bool,
    log_level_override: Option<&Level>,
) -> anyhow::Result<()> {
    let enable_metrics = otel_config
        .enable_metrics
        .unwrap_or(otel_config.enable_observability);

    let normalized_service_name = service_name.to_kebab_case();

    if enable_metrics {
        metrics::configure_metrics(&normalized_service_name, otel_config)?;
    }

    traces::configure_tracing(
        &normalized_service_name,
        otel_config,
        use_structured_logging,
        log_level_override,
    )
}
