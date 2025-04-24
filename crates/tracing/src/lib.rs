use std::path::Path;

#[cfg(feature = "otel")]
use heck::ToKebabCase;
#[cfg(feature = "otel")]
pub use opentelemetry::{
    global,
    metrics::{Counter, Gauge, Histogram, Meter, ObservableGauge, UpDownCounter},
    InstrumentationScope, KeyValue,
};
use wasmcloud_core::logging::Level;
#[cfg(feature = "otel")]
use wasmcloud_core::tls;
use wasmcloud_core::OtelConfig;

#[cfg(feature = "otel")]
pub mod context;
#[cfg(feature = "otel")]
pub mod http;

mod traces;

#[cfg(feature = "otel")]
pub use traces::FlushGuard;

mod metrics;

#[cfg(not(feature = "otel"))]
pub fn configure_observability(
    _: &str,
    _: &OtelConfig,
    use_structured_logging: bool,
    flame_graph: Option<impl AsRef<Path>>,
    log_level_override: Option<&Level>,
) -> anyhow::Result<(tracing::Dispatch, traces::FlushGuard)> {
    // if OTEL is not enabled, explicitly do not emit observability
    let otel_config = OtelConfig::default();
    traces::configure_tracing(
        "",
        &otel_config,
        use_structured_logging,
        flame_graph,
        log_level_override,
    )
}

/// Configures observability for each type of signal
#[cfg(feature = "otel")]
pub fn configure_observability(
    service_name: &str,
    otel_config: &OtelConfig,
    use_structured_logging: bool,
    flame_graph: Option<impl AsRef<Path>>,
    log_level_override: Option<&Level>,
    trace_level_override: Option<&Level>,
) -> anyhow::Result<(tracing::Dispatch, traces::FlushGuard)> {
    let normalized_service_name = service_name.to_kebab_case();

    if otel_config.metrics_enabled() {
        metrics::configure_metrics(&normalized_service_name, otel_config)?;
    }

    traces::configure_tracing(
        &normalized_service_name,
        otel_config,
        use_structured_logging,
        flame_graph,
        log_level_override,
        trace_level_override,
    )
}

/// Configures a reqwest http client with additional certificates
#[cfg(feature = "otel")]
pub(crate) fn get_http_client(otel_config: &OtelConfig) -> anyhow::Result<reqwest::Client> {
    let mut certs = tls::NATIVE_ROOTS.to_vec();
    if !otel_config.additional_ca_paths.is_empty() {
        let additional_certs =
            wasmcloud_core::tls::load_certs_from_paths(&otel_config.additional_ca_paths)
                .unwrap_or_default();
        certs.extend(additional_certs);
    }

    let builder = certs
        .iter()
        .filter_map(|cert| reqwest::tls::Certificate::from_der(cert.as_ref()).ok())
        .fold(reqwest::ClientBuilder::default(), |builder, cert| {
            builder.add_root_certificate(cert)
        });

    Ok(builder
        .user_agent(tls::REQWEST_USER_AGENT)
        .build()
        .expect("failed to build HTTP client"))
}
