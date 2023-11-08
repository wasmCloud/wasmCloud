use std::collections::HashMap;

use opentelemetry::{
    global,
    metrics::{Counter, Histogram, ObservableGauge},
    runtime,
    sdk::metrics::{MeterProvider, PeriodicReader},
    KeyValue,
};
use opentelemetry_otlp::WithExportConfig;
use prometheus::Registry;

pub const DEFAULT_OTEL_METRICS_ENDPOINT: &str = "http://localhost:55681/v1/metrics";
pub const DEFAULT_PROMETHEUS_PORT: i32 = 9090;

const ACTOR_INVOCATIONS: &str = "wasmcloud_actor_invocations";
const ACTOR_ERRORS: &str = "wasmcloud_actor_errors";
const ACTOR_SCALE: &str = "wasmcloud_actor_scale";
const ACTOR_REQUEST_LATENCY_SEC: &str = "wasmcloud_actor_scale";
const PROVIDER_SCALE: &str = "wasmcloud_provider_scale";
const HOST_CACHE_SIZE_KB: &str = "wasmcloud_host_cache_size_kb";
const HOST_OCI_CACHE_SIZE_KB: &str = "wasmcloud_host_oci_cache_size_kb";
const HOST_UPTIME_SEC: &str = "wasmcloud_host_uptime_sec";
const HOST_LABELS: &str = "wasmcloud_host_labels";
const HOST_INFORMATION: &str = "wasmcloud_host_information";
// TODO
// const ACTOR_EXECUTION_TIME_US: &str = "wasmcloud_actor_execution_time_us";
// const ACTOR_QUEUE_TIME_MS: &str = "wasmcloud_actor_queue_time_ms";
// const ACTOR_WAIT_TIME_MS: &str = "wasmcloud_actor_wait_time_ms";

#[derive(Clone, Debug)]
/// `MetricBackend` supports otlp and prometheus exporters
pub enum MetricBackend {
    /// OTLP supports an endpoint and port to send metrics to
    Otlp(String, u32),
    /// Prometheus supports a port to listen on and defaults to 9090
    Prometheus(u32),
    /// Debug sends metrics to stdout
    Debug,
}

#[derive(Debug)]
// Metrics contains the set of metrics wasmcloud can export
pub struct Metrics {
    meter_provider: MeterProvider,
    /// The number of invocations per actor
    pub actor_invocations: Counter<u64>,
    /// The number of errors per actor
    pub actor_errors: Counter<u64>,
    /// The max scale per actor
    pub actor_scale: ObservableGauge<f64>,
    /// The per actor latency of requests they're handling
    pub actor_request_latency_sec: Histogram<f64>,
    /// The max scale per provider
    pub provider_scale: ObservableGauge<f64>,
    /// The wasmcloud host cache size on disk
    pub host_cache_size_kb: Counter<u64>,
    /// The wasmcloud host oci cache size on disk
    pub host_oci_cache_size_kb: Counter<u64>,
    /// The wasmcloud host uptime in seconds
    pub host_uptime_sec: Counter<u64>,
    /// An always-zero counter with all host labels set
    pub host_labels: Counter<u64>,
    /// An always-zero coutner that contains information about the host
    pub host_information: Counter<u64>,
}

fn otlp_metrics() -> MeterProvider {
    let export_config = opentelemetry_otlp::ExportConfig {
        endpoint: "http://localhost:4318/v1/metrics".to_string(),
        ..opentelemetry_otlp::ExportConfig::default()
    };
    opentelemetry_otlp::new_pipeline()
        .metrics(opentelemetry::sdk::runtime::Tokio)
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .http()
                .with_export_config(export_config),
        )
        .build()
        .unwrap()
}

fn prometheus_metrics() -> MeterProvider {
    let registry = Registry::new();
    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(registry.clone())
        .build()
        .unwrap();
    MeterProvider::builder().with_reader(exporter).build()
}

fn debug_metrics() -> MeterProvider {
    MeterProvider::builder()
        .with_reader(
            PeriodicReader::builder(
                opentelemetry_stdout::MetricsExporter::default(),
                runtime::Tokio,
            )
            .build(),
        )
        .build()
}

fn provider(backend: MetricBackend) -> MeterProvider {
    match backend {
        MetricBackend::Otlp(_, _) => otlp_metrics(),
        MetricBackend::Prometheus(_) => prometheus_metrics(),
        MetricBackend::Debug => debug_metrics(),
    }
}

impl Metrics {
    /// Create a new Metrics backend preconfigured with labels
    #[must_use]
    pub fn new(backend: MetricBackend, labels: &HashMap<String, String>) -> Self {
        let provider = provider(backend);
        let meter = global::meter("wasmcloud");
        let host_labels = meter.u64_counter(HOST_LABELS).init();
        let mut otel_labels: Vec<KeyValue> = vec![];

        for (label, value) in labels {
            otel_labels.push(KeyValue::new(label.clone(), value.clone()));
        }
        host_labels.add(0, &otel_labels);
        Metrics {
            meter_provider: provider,
            actor_invocations: meter.u64_counter(ACTOR_INVOCATIONS).init(),
            actor_errors: meter.u64_counter(ACTOR_ERRORS).init(),
            actor_scale: meter.f64_observable_gauge(ACTOR_SCALE).init(),
            actor_request_latency_sec: meter.f64_histogram(ACTOR_REQUEST_LATENCY_SEC).init(),
            provider_scale: meter.f64_observable_gauge(PROVIDER_SCALE).init(),
            host_cache_size_kb: meter.u64_counter(HOST_CACHE_SIZE_KB).init(),
            host_oci_cache_size_kb: meter.u64_counter(HOST_OCI_CACHE_SIZE_KB).init(),
            host_uptime_sec: meter.u64_counter(HOST_UPTIME_SEC).init(),
            host_labels,
            host_information: meter.u64_counter(HOST_INFORMATION).init(),
        }
    }
    /// Shutdown the metrics subsystem, flushing any metrics in flight
    pub fn shutdown(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.meter_provider.shutdown()?;
        Ok(())
    }
}
