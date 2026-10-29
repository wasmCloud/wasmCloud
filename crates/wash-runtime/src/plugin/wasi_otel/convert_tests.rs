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
        extract_counter_values(&metrics),
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
