//! Conversion utilities for WASI OpenTelemetry types

use opentelemetry::logs::{AnyValue, LogRecord as OtelLogRecord};
use opentelemetry::trace::{
    Link, SpanContext, SpanId, SpanKind, Status, TraceContextExt, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{Context, Key, KeyValue};
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
        trace_state: otel_trace_state_to_wit(ctx.trace_state()),
    }
}

/// Convert an OTel `TraceState` into the WIT list-of-tuples representation.
///
/// `TraceState` exposes its entries only via the W3C header form
/// (`key1=value1,key2=value2`), so we render that and split it back into pairs.
fn otel_trace_state_to_wit(trace_state: &TraceState) -> Vec<(String, String)> {
    let header = trace_state.header();
    if header.is_empty() {
        return vec![];
    }
    header
        .split(',')
        .filter_map(|entry| {
            let (key, value) = entry.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
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

/// One data point of a guest sum.
#[derive(Debug)]
pub struct CounterValue {
    pub scope: String,
    pub name: String,
    pub value: f64,
    pub is_monotonic: bool,
    pub attributes: Vec<(String, String)>,
    /// When the guest started this running total, in seconds and nanoseconds.
    /// `None` when the value is an increment rather than a total.
    pub series_start: Option<(u64, u32)>,
}

/// Extract counter/sum values from ResourceMetrics for recording via SDK instruments
pub fn extract_counter_values(metrics: &wasi_metrics::ResourceMetrics) -> Vec<CounterValue> {
    let mut values = Vec::new();

    for scope in &metrics.scope_metrics {
        for metric in &scope.metrics {
            match &metric.data {
                wasi_metrics::MetricData::F64Sum(s)
                | wasi_metrics::MetricData::U64Sum(s)
                | wasi_metrics::MetricData::S64Sum(s) => {
                    // Low-memory makes synchronous counters delta and leaves
                    // up-down counters cumulative.
                    let series_start = match (s.temporality, s.is_monotonic) {
                        (wasi_metrics::Temporality::Cumulative, _)
                        | (wasi_metrics::Temporality::LowMemory, false) => {
                            Some((s.start_time.seconds, s.start_time.nanoseconds))
                        }
                        (wasi_metrics::Temporality::Delta, _)
                        | (wasi_metrics::Temporality::LowMemory, true) => None,
                    };
                    for point in &s.data_points {
                        let attributes: Vec<(String, String)> = point
                            .attributes
                            .iter()
                            .map(|kv| (kv.key.clone(), kv.value.clone()))
                            .collect();
                        values.push(CounterValue {
                            scope: scope.scope.name.clone(),
                            name: metric.name.clone(),
                            value: metric_number_to_f64(&point.value),
                            is_monotonic: s.is_monotonic,
                            attributes,
                            series_start,
                        });
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
pub(super) fn convert_datetime(
    dt: &super::bindings::wasi::clocks::wall_clock::Datetime,
) -> SystemTime {
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

/// Convert WASI trace flags to OTel `TraceFlags`, preserving the sampled bit.
fn wit_trace_flags_to_otel(flags: WitTraceFlags) -> TraceFlags {
    if flags.contains(WitTraceFlags::SAMPLED) {
        TraceFlags::SAMPLED
    } else {
        TraceFlags::default()
    }
}

/// Convert WASI span context to OTel SpanContext
pub fn wit_span_context_to_otel(ctx: &WitSpanContext) -> SpanContext {
    let trace_id = TraceId::from_hex(&ctx.trace_id).unwrap_or(TraceId::INVALID);
    let span_id = SpanId::from_hex(&ctx.span_id).unwrap_or(SpanId::INVALID);
    let trace_flags = wit_trace_flags_to_otel(ctx.trace_flags);

    // Convert trace state from list of tuples
    let trace_state = TraceState::from_key_value(
        ctx.trace_state
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str())),
    )
    .unwrap_or_default();

    SpanContext::new(trace_id, span_id, trace_flags, ctx.is_remote, trace_state)
}

/// Build the parent [`Context`] for a WASI span from its `parent_span_id`.
///
/// The guest already tracks its own span nesting (e.g. `compute` parented under
/// `handler`); that relationship is carried over the wire as `parent_span_id` on
/// `SpanData`. Without using it here, `Tracer::build` falls back to whatever OTel
/// context is ambient on the host when `on-end` is processed, which flattens every
/// guest span onto the host's current span instead of preserving guest-side nesting.
pub fn wasi_span_parent_context(span: &wasi_tracing::SpanData) -> Context {
    let Ok(parent_span_id) = SpanId::from_hex(&span.parent_span_id) else {
        return Context::new();
    };
    if parent_span_id == SpanId::INVALID {
        return Context::new();
    }

    let trace_id = TraceId::from_hex(&span.span_context.trace_id).unwrap_or(TraceId::INVALID);
    let trace_flags = wit_trace_flags_to_otel(span.span_context.trace_flags);

    let parent_span_context = SpanContext::new(
        trace_id,
        parent_span_id,
        trace_flags,
        true,
        TraceState::default(),
    );

    Context::new().with_remote_span_context(parent_span_context)
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
        wasi_tracing::Status::Error(msg) => format!("error: {msg}"),
    };

    SpanSummary {
        name: span.name.clone(),
        trace_id: span.span_context.trace_id.clone(),
        span_id: span.span_context.span_id.clone(),
        parent_span_id: span.parent_span_id.clone(),
        kind: kind.to_string(),
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

/// Extract span links, preserving each link's span context and attributes.
pub fn extract_span_links(span: &wasi_tracing::SpanData) -> Vec<Link> {
    span.links
        .iter()
        .map(|link| {
            Link::new(
                wit_span_context_to_otel(&link.span_context),
                convert_key_values(&link.attributes),
                0,
            )
        })
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::plugin::wasi_otel::bindings::wasi::{
        clocks::wall_clock::Datetime,
        otel::{metrics as m, tracing as t, types},
    };

    fn scope() -> types::InstrumentationScope {
        types::InstrumentationScope {
            name: "guest".into(),
            version: Some("1.2.3".into()),
            schema_url: None,
            attributes: vec![],
        }
    }
    fn attrs() -> Vec<types::KeyValue> {
        vec![types::KeyValue {
            key: "route".into(),
            value: "\"/hello\"".into(),
        }]
    }
    fn time() -> Datetime {
        Datetime {
            seconds: 10,
            nanoseconds: 20,
        }
    }
    fn context() -> WitSpanContext {
        WitSpanContext {
            trace_id: "0123456789abcdef0123456789abcdef".into(),
            span_id: "0123456789abcdef".into(),
            trace_flags: WitTraceFlags::SAMPLED,
            is_remote: true,
            trace_state: vec![("vendor".into(), "state".into())],
        }
    }

    #[test]
    fn span_context_round_trip_preserves_identity_flags_and_state() {
        let input = context();
        let converted = wit_span_context_to_otel(&input);
        let output = otel_span_context_to_wit(&converted);
        assert_eq!(output.trace_id, input.trace_id);
        assert_eq!(output.span_id, input.span_id);
        assert_eq!(output.trace_state, input.trace_state);
        assert!(output.is_remote);
        assert!(output.trace_flags.contains(WitTraceFlags::SAMPLED));
        let invalid = WitSpanContext {
            trace_id: "invalid".into(),
            span_id: "invalid".into(),
            trace_flags: WitTraceFlags::empty(),
            is_remote: false,
            trace_state: vec![("invalid key".into(), "state".into())],
        };
        let converted = wit_span_context_to_otel(&invalid);
        assert!(!converted.is_valid());
        assert!(!converted.is_sampled());
        assert!(otel_span_context_to_wit(&converted).trace_state.is_empty());
    }

    #[test]
    fn span_conversion_preserves_parent_events_links_and_status() {
        let mut span = t::SpanData {
            span_context: context(),
            parent_span_id: "fedcba9876543210".into(),
            span_kind: t::SpanKind::Server,
            name: "request".into(),
            start_time: time(),
            end_time: time(),
            attributes: attrs(),
            events: vec![t::Event {
                name: "ready".into(),
                time: time(),
                attributes: attrs(),
            }],
            links: vec![t::Link {
                span_context: context(),
                attributes: attrs(),
            }],
            status: t::Status::Ok,
            instrumentation_scope: scope(),
            dropped_attributes: 0,
            dropped_events: 0,
            dropped_links: 0,
        };
        assert_eq!(
            wasi_span_parent_context(&span)
                .span()
                .span_context()
                .span_id()
                .to_string(),
            "fedcba9876543210"
        );
        assert_eq!(
            extract_span_attributes(&span),
            vec![KeyValue::new("route", "\"/hello\"")]
        );
        let events = extract_span_events(&span);
        assert_eq!(
            events,
            vec![(
                "ready".into(),
                UNIX_EPOCH + Duration::new(10, 20),
                vec![KeyValue::new("route", "\"/hello\"")]
            )]
        );
        let links = extract_span_links(&span);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links.first().map(|l| l.span_context.clone()),
            Some(wit_span_context_to_otel(&context()))
        );
        assert_eq!(
            links.first().map(|l| l.attributes.clone()),
            Some(vec![KeyValue::new("route", "\"/hello\"")])
        );
        for (kind, expected, label) in [
            (t::SpanKind::Client, SpanKind::Client, "client"),
            (t::SpanKind::Server, SpanKind::Server, "server"),
            (t::SpanKind::Producer, SpanKind::Producer, "producer"),
            (t::SpanKind::Consumer, SpanKind::Consumer, "consumer"),
            (t::SpanKind::Internal, SpanKind::Internal, "internal"),
        ] {
            assert_eq!(convert_span_kind(kind), expected);
            span.span_kind = kind;
            let summary = summarize_span_data(&span);
            assert_eq!(summary.name, "request");
            assert_eq!(summary.trace_id, context().trace_id);
            assert_eq!(summary.span_id, context().span_id);
            assert_eq!(summary.parent_span_id, span.parent_span_id);
            assert_eq!(summary.kind, label);
            assert_eq!(
                (
                    summary.attribute_count,
                    summary.event_count,
                    summary.link_count
                ),
                (1, 1, 1)
            );
        }
        for (status, expected, label) in [
            (t::Status::Unset, Status::Unset, "unset"),
            (t::Status::Ok, Status::Ok, "ok"),
            (
                t::Status::Error("failed".into()),
                Status::error("failed"),
                "error: failed",
            ),
        ] {
            assert_eq!(convert_status(&status), expected);
            span.status = status;
            assert_eq!(summarize_span_data(&span).status, label);
        }
        for parent in ["invalid", "0000000000000000"] {
            span.parent_span_id = parent.into();
            assert!(
                !wasi_span_parent_context(&span)
                    .span()
                    .span_context()
                    .is_valid()
            );
        }
    }

    #[test]
    fn metric_extraction_separates_gauges_from_monotonic_and_signed_sums() {
        let gauge = |value| m::Gauge {
            data_points: vec![m::GaugeDataPoint {
                attributes: attrs(),
                value,
                exemplars: vec![],
            }],
            start_time: None,
            time: time(),
        };
        let sum = |value, is_monotonic| m::Sum {
            data_points: vec![m::SumDataPoint {
                attributes: attrs(),
                value,
                exemplars: vec![],
            }],
            start_time: time(),
            time: time(),
            temporality: m::Temporality::Cumulative,
            is_monotonic,
        };
        let histogram = || m::Histogram {
            data_points: vec![],
            start_time: time(),
            time: time(),
            temporality: m::Temporality::Delta,
        };
        let exponential = || m::ExponentialHistogram {
            data_points: vec![],
            start_time: time(),
            time: time(),
            temporality: m::Temporality::Delta,
        };
        let data = vec![
            m::MetricData::F64Gauge(gauge(m::MetricNumber::F64(1.5))),
            m::MetricData::S64Gauge(gauge(m::MetricNumber::S64(-2))),
            m::MetricData::U64Gauge(gauge(m::MetricNumber::U64(3))),
            m::MetricData::F64Sum(sum(m::MetricNumber::F64(1.5), true)),
            m::MetricData::S64Sum(sum(m::MetricNumber::S64(-2), false)),
            m::MetricData::U64Sum(sum(m::MetricNumber::U64(3), true)),
            m::MetricData::F64Histogram(histogram()),
            m::MetricData::S64Histogram(histogram()),
            m::MetricData::U64Histogram(histogram()),
            m::MetricData::F64ExponentialHistogram(exponential()),
            m::MetricData::S64ExponentialHistogram(exponential()),
            m::MetricData::U64ExponentialHistogram(exponential()),
        ];
        let metrics = m::ResourceMetrics {
            resource: types::Resource {
                attributes: vec![],
                schema_url: None,
            },
            scope_metrics: vec![m::ScopeMetrics {
                scope: scope(),
                metrics: data
                    .into_iter()
                    .enumerate()
                    .map(|(i, data)| m::Metric {
                        name: format!("metric-{i}"),
                        description: String::new(),
                        unit: String::new(),
                        data,
                    })
                    .collect(),
            }],
        };
        let summary = summarize_resource_metrics(&metrics);
        assert_eq!((summary.total_scopes, summary.total_metrics), (1, 12));
        assert!(
            summary
                .metric_names
                .contains(&"metric-4[sum(1 points, non-monotonic)]".to_string())
        );
        assert!(
            summary
                .metric_names
                .contains(&"metric-6[histogram(0 points)]".to_string())
        );
        assert!(
            summary
                .metric_names
                .contains(&"metric-9[exp_histogram(0 points)]".to_string())
        );
        let attributes = vec![("route".into(), "\"/hello\"".into())];
        assert_eq!(
            extract_gauge_values(&metrics),
            vec![
                ("metric-0".into(), 1.5, attributes.clone()),
                ("metric-1".into(), -2.0, attributes.clone()),
                ("metric-2".into(), 3.0, attributes.clone())
            ]
        );
        assert_eq!(
            extract_counter_values(&metrics)
                .into_iter()
                .map(|counter| (
                    counter.name,
                    counter.value,
                    counter.is_monotonic,
                    counter.attributes
                ))
                .collect::<Vec<_>>(),
            vec![
                ("metric-3".into(), 1.5, true, attributes.clone()),
                ("metric-4".into(), -2.0, false, attributes.clone()),
                ("metric-5".into(), 3.0, true, attributes)
            ]
        );
    }

    #[test]
    fn log_conversion_preserves_severity_timestamps_and_attributes() {
        use opentelemetry::logs::{Logger as _, LoggerProvider as _, Severity};
        let provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder().build();
        let logger = provider.logger("test");
        for (number, expected) in [
            (0, Severity::Info),
            (1, Severity::Trace),
            (5, Severity::Debug),
            (9, Severity::Info),
            (13, Severity::Warn),
            (17, Severity::Error),
            (21, Severity::Fatal),
            (255, Severity::Info),
        ] {
            let mut output = logger.create_log_record();
            let record = WasiLogRecord {
                timestamp: Some(time()),
                observed_timestamp: Some(time()),
                severity_text: None,
                severity_number: Some(number),
                body: Some("message".into()),
                attributes: Some(attrs()),
                event_name: None,
                resource: Some(types::Resource {
                    attributes: attrs(),
                    schema_url: None,
                }),
                instrumentation_scope: Some(scope()),
                trace_id: Some(context().trace_id),
                span_id: Some(context().span_id),
                trace_flags: Some(WitTraceFlags::SAMPLED),
            };
            convert_wasi_log_record(record, &mut output, "service");
            assert_eq!(output.severity_number(), Some(expected));
            assert_eq!(output.timestamp(), Some(UNIX_EPOCH + Duration::new(10, 20)));
            assert_eq!(output.observed_timestamp(), output.timestamp());
            assert_eq!(output.body(), Some(&AnyValue::String("message".into())));
            assert_eq!(output.attributes_iter().count(), 5);
            assert_eq!(
                output.trace_context().map(|c| c.trace_id.to_string()),
                Some(context().trace_id)
            );
        }
        let mut output = logger.create_log_record();
        convert_wasi_log_record(
            WasiLogRecord {
                timestamp: None,
                observed_timestamp: None,
                severity_text: None,
                severity_number: None,
                body: None,
                attributes: None,
                event_name: None,
                resource: None,
                instrumentation_scope: None,
                trace_id: Some("invalid".into()),
                span_id: None,
                trace_flags: None,
            },
            &mut output,
            "service",
        );
        assert_eq!(output.body(), None);
        assert_eq!(output.timestamp(), None);
        assert_eq!(
            output.trace_context().map(|c| c.trace_id),
            Some(TraceId::INVALID)
        );
    }
}
