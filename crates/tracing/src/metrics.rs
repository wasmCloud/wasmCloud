#[cfg(feature = "otel")]
use anyhow::Context;

#[cfg(feature = "otel")]
#[allow(clippy::missing_errors_doc)]
#[allow(clippy::module_name_repetitions)]
pub fn configure_metrics(
    service_name: &str,
    otel_config: &wasmcloud_core::OtelConfig,
) -> anyhow::Result<()> {
    use opentelemetry_otlp::{MetricsExporterBuilder, WithExportConfig};
    use wasmcloud_core::OtelProtocol;

    let builder: MetricsExporterBuilder = match otel_config.protocol {
        OtelProtocol::Http => {
            let client = crate::get_http_client(otel_config)
                .context("failed to get an http client for otel metrics exporter")?;
            opentelemetry_otlp::new_exporter()
                .http()
                .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
                .with_http_client(client)
                .with_endpoint(otel_config.metrics_endpoint())
                .into()
        }
        OtelProtocol::Grpc => {
            // TODO(joonas): Configure tonic::transport::ClientTlsConfig via .with_tls_config(...), passing in additional certificates.
            opentelemetry_otlp::new_exporter()
                .tonic()
                .with_endpoint(otel_config.metrics_endpoint())
                .into()
        }
    };

    opentelemetry_otlp::new_pipeline()
        .metrics(opentelemetry_sdk::runtime::Tokio)
        .with_exporter(builder)
        .with_resource(opentelemetry_sdk::Resource::new(vec![
            opentelemetry::KeyValue::new("service.name", service_name.to_string()),
        ]))
        .with_aggregation_selector(ExponentialHistogramAggregationSelector::new())
        .with_temporality_selector(
            opentelemetry_sdk::metrics::reader::DefaultTemporalitySelector::new(),
        )
        .build()
        .context("failed to create OTEL metrics provider")?;

    Ok(())
}

/// Replaces the default `ExplicitBucketHistogram` aggregation for Histograms
/// with `Base2ExponentialHistogram`. This makes it easier to capture latency
/// at nanosecond accuracy.
#[cfg(feature = "otel")]
#[derive(Clone, Default, Debug)]
struct ExponentialHistogramAggregationSelector {
    pub(crate) _private: (),
}

#[cfg(feature = "otel")]
impl ExponentialHistogramAggregationSelector {
    /// Create a new  aggregation selector.
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(feature = "otel")]
impl opentelemetry_sdk::metrics::reader::AggregationSelector
    for ExponentialHistogramAggregationSelector
{
    fn aggregation(
        &self,
        kind: opentelemetry_sdk::metrics::InstrumentKind,
    ) -> opentelemetry_sdk::metrics::Aggregation {
        match kind {
            opentelemetry_sdk::metrics::InstrumentKind::Counter
            | opentelemetry_sdk::metrics::InstrumentKind::UpDownCounter
            | opentelemetry_sdk::metrics::InstrumentKind::ObservableCounter
            | opentelemetry_sdk::metrics::InstrumentKind::ObservableUpDownCounter => {
                opentelemetry_sdk::metrics::Aggregation::Sum
            }
            opentelemetry_sdk::metrics::InstrumentKind::Gauge
            | opentelemetry_sdk::metrics::InstrumentKind::ObservableGauge => {
                opentelemetry_sdk::metrics::Aggregation::LastValue
            }
            opentelemetry_sdk::metrics::InstrumentKind::Histogram => {
                opentelemetry_sdk::metrics::Aggregation::Base2ExponentialHistogram {
                    max_size: 160,
                    max_scale: 20,
                    record_min_max: true,
                }
            }
        }
    }
}
