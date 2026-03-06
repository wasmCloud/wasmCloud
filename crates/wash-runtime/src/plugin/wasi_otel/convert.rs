//! Conversion utilities for WASI OpenTelemetry types

use opentelemetry::logs::{AnyValue, LogRecord as OtelLogRecord};
use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{Key, KeyValue};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::bindings::wasi::otel::logs::LogRecord as WasiLogRecord;
use super::bindings::wasi::otel::metrics as wasi_metrics;
use super::bindings::wasi::otel::tracing as wasi_tracing;
use super::bindings::wasi::otel::tracing::{
    SpanContext as WitSpanContext, TraceFlags as WitTraceFlags,
};

/// Convert OTel span context to WIT
pub fn otel_span_context_to_wit(ctx: &SpanContext) -> WitSpanContext {
    WitSpanContext {
        trace_id: format!("{:032x}", ctx.trace_id()),
        span_id: format!("{:016x}", ctx.span_id()),
        trace_flags: if ctx.is_sampled() {
            WitTraceFlags::SAMPLED
        } else {
            WitTraceFlags::empty()
        },
        is_remote: ctx.is_remote(),
        trace_state: vec![],
    }
}

/// Converts a WASI OTEL LogRecord to populate an OpenTelemetry LogRecord
pub fn convert_wasi_log_record<R: OtelLogRecord>(
    wasi_record: WasiLogRecord,
    otel_record: &mut R,
    service_name: impl Into<String>,
) {
    use opentelemetry::logs::Severity;

    otel_record.add_attribute(
        Key::new("resource.service.name"),
        AnyValue::String(service_name.into().into()),
    );

    // Set timestamp
    if let Some(ts) = wasi_record.timestamp {
        let duration = Duration::new(ts.seconds, ts.nanoseconds);
        if let Some(time) = UNIX_EPOCH.checked_add(duration) {
            otel_record.set_timestamp(time);
        }
    }

    // Set observed timestamp
    if let Some(ts) = wasi_record.observed_timestamp {
        let duration = Duration::new(ts.seconds, ts.nanoseconds);
        if let Some(time) = UNIX_EPOCH.checked_add(duration) {
            otel_record.set_observed_timestamp(time);
        }
    }

    // Set severity number (map u8 to Severity enum)
    if let Some(severity_num) = wasi_record.severity_number {
        let severity = match severity_num {
            1..=4 => Severity::Trace,
            5..=8 => Severity::Debug,
            9..=12 => Severity::Info,
            13..=16 => Severity::Warn,
            17..=20 => Severity::Error,
            21..=24 => Severity::Fatal,
            _ => Severity::Info, // Default fallback
        };
        otel_record.set_severity_number(severity);
    }

    // Set body (value is a JSON-encoded string in WASI OTEL)
    if let Some(ref body) = wasi_record.body {
        otel_record.set_body(AnyValue::String(body.clone().into()));
    }

    // Set attributes
    if let Some(ref attributes) = wasi_record.attributes {
        for kv in attributes {
            // The value in WASI OTEL is a JSON-encoded string
            otel_record.add_attribute(
                Key::new(kv.key.clone()),
                AnyValue::String(kv.value.clone().into()),
            );
        }
    }

    // Set trace context (trace_id, span_id, trace_flags)
    if wasi_record.trace_id.is_some() || wasi_record.span_id.is_some() {
        let trace_id = wasi_record
            .trace_id
            .as_ref()
            .and_then(|id| TraceId::from_hex(id).ok())
            .unwrap_or(TraceId::INVALID);

        let span_id = wasi_record
            .span_id
            .as_ref()
            .and_then(|id| SpanId::from_hex(id).ok())
            .unwrap_or(SpanId::INVALID);

        let flags = wasi_record
            .trace_flags
            .map(|f| {
                if f.contains(super::bindings::wasi::otel::tracing::TraceFlags::SAMPLED) {
                    TraceFlags::SAMPLED
                } else {
                    TraceFlags::default()
                }
            })
            .unwrap_or_default();

        otel_record.set_trace_context(trace_id, span_id, Some(flags));
    }

    // Note: resource and instrumentation_scope from the WASI record are typically
    // handled at the Logger/LoggerProvider level in OpenTelemetry, not on individual records.
    // If needed, they can be added as attributes:
    if let Some(ref resource) = wasi_record.resource {
        for kv in &resource.attributes {
            otel_record.add_attribute(
                Key::new(format!("resource.{}", kv.key)),
                AnyValue::String(kv.value.clone().into()),
            );
        }
    }

    if let Some(ref scope) = wasi_record.instrumentation_scope {
        otel_record.add_attribute(
            Key::new("instrumentation_scope.name"),
            AnyValue::String(scope.name.clone().into()),
        );
        if let Some(ref version) = scope.version {
            otel_record.add_attribute(
                Key::new("instrumentation_scope.version"),
                AnyValue::String(version.clone().into()),
            );
        }
    }
}

// ============================================================================
// Metrics Summary (for logging/debugging)
// ============================================================================

/// Summary of a WASI ResourceMetrics for logging purposes
pub struct MetricsSummary {
    pub total_scopes: usize,
    pub total_metrics: usize,
    pub metric_names: Vec<String>,
}

/// Convert WASI metric number to f64
fn metric_number_to_f64(n: &wasi_metrics::MetricNumber) -> f64 {
    match n {
        wasi_metrics::MetricNumber::F64(v) => *v,
        wasi_metrics::MetricNumber::S64(v) => *v as f64,
        wasi_metrics::MetricNumber::U64(v) => *v as f64,
    }
}

/// Get a summary of the metric data type
fn metric_data_summary(data: &wasi_metrics::MetricData) -> String {
    match data {
        wasi_metrics::MetricData::F64Gauge(g)
        | wasi_metrics::MetricData::U64Gauge(g)
        | wasi_metrics::MetricData::S64Gauge(g) => {
            format!("gauge({} points)", g.data_points.len())
        }
        wasi_metrics::MetricData::F64Sum(s)
        | wasi_metrics::MetricData::U64Sum(s)
        | wasi_metrics::MetricData::S64Sum(s) => {
            let monotonic = if s.is_monotonic {
                "monotonic"
            } else {
                "non-monotonic"
            };
            format!("sum({} points, {})", s.data_points.len(), monotonic)
        }
        wasi_metrics::MetricData::F64Histogram(h)
        | wasi_metrics::MetricData::U64Histogram(h)
        | wasi_metrics::MetricData::S64Histogram(h) => {
            format!("histogram({} points)", h.data_points.len())
        }
        wasi_metrics::MetricData::F64ExponentialHistogram(h)
        | wasi_metrics::MetricData::U64ExponentialHistogram(h)
        | wasi_metrics::MetricData::S64ExponentialHistogram(h) => {
            format!("exp_histogram({} points)", h.data_points.len())
        }
    }
}

/// Summarize WASI ResourceMetrics for logging
pub fn summarize_resource_metrics(metrics: &wasi_metrics::ResourceMetrics) -> MetricsSummary {
    let total_scopes = metrics.scope_metrics.len();
    let mut total_metrics = 0;
    let mut metric_names = Vec::new();

    for scope in &metrics.scope_metrics {
        for metric in &scope.metrics {
            total_metrics += 1;
            metric_names.push(format!(
                "{}[{}]",
                metric.name,
                metric_data_summary(&metric.data)
            ));
        }
    }

    MetricsSummary {
        total_scopes,
        total_metrics,
        metric_names,
    }
}

/// Gauge value: (metric_name, value, attributes)
type GaugeValue = (String, f64, Vec<(String, String)>);

/// Extract gauge values from ResourceMetrics for recording via SDK instruments
pub fn extract_gauge_values(metrics: &wasi_metrics::ResourceMetrics) -> Vec<GaugeValue> {
    let mut values = Vec::new();

    for scope in &metrics.scope_metrics {
        for metric in &scope.metrics {
            match &metric.data {
                wasi_metrics::MetricData::F64Gauge(g)
                | wasi_metrics::MetricData::U64Gauge(g)
                | wasi_metrics::MetricData::S64Gauge(g) => {
                    for point in &g.data_points {
                        let attrs: Vec<(String, String)> = point
                            .attributes
                            .iter()
                            .map(|kv| (kv.key.clone(), kv.value.clone()))
                            .collect();
                        values.push((
                            metric.name.clone(),
                            metric_number_to_f64(&point.value),
                            attrs,
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    values
}

/// Counter value: (metric_name, value, is_monotonic, attributes)
type CounterValue = (String, f64, bool, Vec<(String, String)>);

/// Extract counter/sum values from ResourceMetrics for recording via SDK instruments
pub fn extract_counter_values(metrics: &wasi_metrics::ResourceMetrics) -> Vec<CounterValue> {
    let mut values = Vec::new();

    for scope in &metrics.scope_metrics {
        for metric in &scope.metrics {
            match &metric.data {
                wasi_metrics::MetricData::F64Sum(s)
                | wasi_metrics::MetricData::U64Sum(s)
                | wasi_metrics::MetricData::S64Sum(s) => {
                    for point in &s.data_points {
                        let attrs: Vec<(String, String)> = point
                            .attributes
                            .iter()
                            .map(|kv| (kv.key.clone(), kv.value.clone()))
                            .collect();
                        values.push((
                            metric.name.clone(),
                            metric_number_to_f64(&point.value),
                            s.is_monotonic,
                            attrs,
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    values
}

// ============================================================================
// Tracing Conversion
// ============================================================================

/// Convert a WASI datetime to SystemTime
fn convert_datetime(dt: &super::bindings::wasi::clocks::wall_clock::Datetime) -> SystemTime {
    let duration = Duration::new(dt.seconds, dt.nanoseconds);
    UNIX_EPOCH.checked_add(duration).unwrap_or(UNIX_EPOCH)
}

/// Convert WASI key-value pairs to OpenTelemetry KeyValue attributes
fn convert_key_values(attrs: &[super::bindings::wasi::otel::types::KeyValue]) -> Vec<KeyValue> {
    attrs
        .iter()
        .map(|kv| KeyValue::new(kv.key.clone(), kv.value.clone()))
        .collect()
}

/// Convert WASI span context to OTel SpanContext
pub fn wit_span_context_to_otel(ctx: &WitSpanContext) -> SpanContext {
    let trace_id = TraceId::from_hex(&ctx.trace_id).unwrap_or(TraceId::INVALID);
    let span_id = SpanId::from_hex(&ctx.span_id).unwrap_or(SpanId::INVALID);
    let trace_flags = if ctx.trace_flags.contains(WitTraceFlags::SAMPLED) {
        TraceFlags::SAMPLED
    } else {
        TraceFlags::default()
    };

    // Convert trace state from list of tuples
    let trace_state = TraceState::from_key_value(
        ctx.trace_state
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str())),
    )
    .unwrap_or_default();

    SpanContext::new(trace_id, span_id, trace_flags, ctx.is_remote, trace_state)
}

/// Convert WASI span kind to OTel SpanKind
pub fn convert_span_kind(kind: wasi_tracing::SpanKind) -> SpanKind {
    match kind {
        wasi_tracing::SpanKind::Client => SpanKind::Client,
        wasi_tracing::SpanKind::Server => SpanKind::Server,
        wasi_tracing::SpanKind::Producer => SpanKind::Producer,
        wasi_tracing::SpanKind::Consumer => SpanKind::Consumer,
        wasi_tracing::SpanKind::Internal => SpanKind::Internal,
    }
}

/// Convert WASI status to OTel Status
pub fn convert_status(status: &wasi_tracing::Status) -> Status {
    match status {
        wasi_tracing::Status::Unset => Status::Unset,
        wasi_tracing::Status::Ok => Status::Ok,
        wasi_tracing::Status::Error(msg) => Status::error(msg.clone()),
    }
}

/// Summary of span data for logging
pub struct SpanSummary {
    pub name: String,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub kind: String,
    pub start_time: SystemTime,
    pub end_time: SystemTime,
    pub attribute_count: usize,
    pub event_count: usize,
    pub link_count: usize,
    pub status: String,
}

/// Summarize WASI SpanData for logging
pub fn summarize_span_data(span: &wasi_tracing::SpanData) -> SpanSummary {
    let kind = match span.span_kind {
        wasi_tracing::SpanKind::Client => "client",
        wasi_tracing::SpanKind::Server => "server",
        wasi_tracing::SpanKind::Producer => "producer",
        wasi_tracing::SpanKind::Consumer => "consumer",
        wasi_tracing::SpanKind::Internal => "internal",
    };

    let status = match &span.status {
        wasi_tracing::Status::Unset => "unset".to_string(),
        wasi_tracing::Status::Ok => "ok".to_string(),
        wasi_tracing::Status::Error(msg) => format!("error: {}", msg),
    };

    SpanSummary {
        name: span.name.clone(),
        trace_id: span.span_context.trace_id.clone(),
        span_id: span.span_context.span_id.clone(),
        parent_span_id: span.parent_span_id.clone(),
        kind: kind.to_string(),
        start_time: convert_datetime(&span.start_time),
        end_time: convert_datetime(&span.end_time),
        attribute_count: span.attributes.len(),
        event_count: span.events.len(),
        link_count: span.links.len(),
        status,
    }
}

/// Extract span attributes as KeyValue vector
pub fn extract_span_attributes(span: &wasi_tracing::SpanData) -> Vec<KeyValue> {
    convert_key_values(&span.attributes)
}

/// Extract span events for recording
pub fn extract_span_events(
    span: &wasi_tracing::SpanData,
) -> Vec<(String, SystemTime, Vec<KeyValue>)> {
    span.events
        .iter()
        .map(|e| {
            (
                e.name.clone(),
                convert_datetime(&e.time),
                convert_key_values(&e.attributes),
            )
        })
        .collect()
}
