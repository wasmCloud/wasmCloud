use std::collections::HashMap;

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

#[derive(Debug)]
pub struct Metrics {
    pub meter: opentelemetry::metrics::Meter,
    pub actor_invocations: opentelemetry::metrics::Counter<u64>,
    pub actor_errors: opentelemetry::metrics::Counter<u64>,
    pub actor_scale: opentelemetry::metrics::ObservableGauge<f64>,
    pub actor_request_latency_sec: opentelemetry::metrics::Histogram<f64>,
    pub provider_scale: opentelemetry::metrics::ObservableGauge<f64>,
    pub host_cache_size_kb: opentelemetry::metrics::Counter<u64>,
    pub host_oci_cache_size_kb: opentelemetry::metrics::Counter<u64>,
    pub host_uptime_sec: opentelemetry::metrics::Counter<u64>,
    pub host_labels: opentelemetry::metrics::Counter<u64>,
    pub host_information: opentelemetry::metrics::Counter<u64>,
}

impl Metrics {
    pub fn new(labels: &HashMap<String, String>) -> Self {
        let meter = opentelemetry::global::meter("wasmcloud");
        let host_labels = meter.u64_counter(HOST_LABELS).init();
        let mut otel_labels: Vec<opentelemetry::KeyValue> = vec![];

        for (label, value) in labels {
            otel_labels.push(opentelemetry::KeyValue::new(label.clone(), value.clone()));
        }
        host_labels.add(0, &otel_labels);
        Metrics {
            meter: meter.clone(),
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
}
