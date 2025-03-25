#[cfg(feature = "otel")]
use anyhow::Context;

#[cfg(feature = "otel")]
#[allow(clippy::missing_errors_doc)]
#[allow(clippy::module_name_repetitions)]
pub fn configure_metrics(
    service_name: &str,
    otel_config: &wasmcloud_core::OtelConfig,
) -> anyhow::Result<()> {
    use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
    use opentelemetry_sdk::metrics::{
        periodic_reader_with_async_runtime::PeriodicReader, SdkMeterProvider,
    };
    use wasmcloud_core::OtelProtocol;

    let exporter = match otel_config.protocol {
        OtelProtocol::Http => {
            let client = crate::get_http_client(otel_config)
                .context("failed to get an http client for otel metrics exporter")?;
            opentelemetry_otlp::MetricExporter::builder()
                .with_http()
                .with_http_client(client)
                .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
                .with_endpoint(otel_config.metrics_endpoint())
                .build()
                .context("failed to create OTEL http exporter")?
        }
        OtelProtocol::Grpc => {
            // TODO(joonas): Configure tonic::transport::ClientTlsConfig via .with_tls_config(...), passing in additional certificates.
            opentelemetry_otlp::MetricExporter::builder()
                .with_tonic()
                .with_endpoint(otel_config.metrics_endpoint())
                .build()
                .context("failed to create OTEL tonic exporter")?
        }
    };

    let reader = PeriodicReader::builder(exporter, opentelemetry_sdk::runtime::Tokio).build();

    let meter_provider = SdkMeterProvider::builder()
        .with_resource(
            opentelemetry_sdk::Resource::builder_empty()
                .with_detector(Box::new(
                    opentelemetry_sdk::resource::EnvResourceDetector::new(),
                ))
                .with_attribute(opentelemetry::KeyValue::new(
                    "service.name",
                    service_name.to_string(),
                ))
                .build(),
        )
        .with_reader(reader)
        .build();

    opentelemetry::global::set_meter_provider(meter_provider);

    Ok(())
}
