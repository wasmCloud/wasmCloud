use super::*;
use bindings::wasi::{
    clocks::wall_clock::Datetime,
    otel::{logs as l, metrics as m, tracing as t, types},
};
use opentelemetry_sdk::{
    logs::InMemoryLogExporter,
    metrics::{InMemoryMetricExporter, PeriodicReader},
    trace::InMemorySpanExporter,
};

fn scope() -> types::InstrumentationScope {
    types::InstrumentationScope {
        name: "guest".into(),
        version: None,
        schema_url: None,
        attributes: vec![],
    }
}

fn time() -> Datetime {
    Datetime {
        seconds: 10,
        nanoseconds: 20,
    }
}

fn span_context() -> t::SpanContext {
    t::SpanContext {
        trace_id: "0123456789abcdef0123456789abcdef".into(),
        span_id: "0123456789abcdef".into(),
        trace_flags: WitTraceFlags::SAMPLED,
        is_remote: false,
        trace_state: vec![],
    }
}

fn span() -> t::SpanData {
    t::SpanData {
        span_context: span_context(),
        parent_span_id: "fedcba9876543210".into(),
        span_kind: t::SpanKind::Server,
        name: "request".into(),
        start_time: time(),
        end_time: time(),
        attributes: vec![types::KeyValue {
            key: "route".into(),
            value: "\"/hello\"".into(),
        }],
        events: vec![t::Event {
            name: "ready".into(),
            time: time(),
            attributes: vec![],
        }],
        links: vec![],
        status: t::Status::Ok,
        instrumentation_scope: scope(),
        dropped_attributes: 0,
        dropped_events: 0,
        dropped_links: 0,
    }
}

fn record(severity: Option<u8>) -> l::LogRecord {
    l::LogRecord {
        timestamp: Some(time()),
        observed_timestamp: Some(time()),
        severity_text: None,
        severity_number: severity,
        body: Some("hello".into()),
        attributes: None,
        event_name: Some("guest-log".into()),
        resource: None,
        instrumentation_scope: Some(scope()),
        trace_id: Some(span_context().trace_id),
        span_id: Some(span_context().span_id),
        trace_flags: Some(WitTraceFlags::SAMPLED),
    }
}

fn metrics() -> m::ResourceMetrics {
    let attributes = || {
        vec![types::KeyValue {
            key: "route".into(),
            value: "\"/hello\"".into(),
        }]
    };
    m::ResourceMetrics {
        resource: types::Resource {
            attributes: vec![],
            schema_url: None,
        },
        scope_metrics: vec![m::ScopeMetrics {
            scope: scope(),
            metrics: vec![
                m::Metric {
                    name: "temperature".into(),
                    description: String::new(),
                    unit: String::new(),
                    data: m::MetricData::F64Gauge(m::Gauge {
                        data_points: vec![m::GaugeDataPoint {
                            attributes: attributes(),
                            value: m::MetricNumber::F64(1.5),
                            exemplars: vec![],
                        }],
                        start_time: None,
                        time: time(),
                    }),
                },
                m::Metric {
                    name: "requests".into(),
                    description: String::new(),
                    unit: String::new(),
                    data: m::MetricData::U64Sum(m::Sum {
                        data_points: vec![m::SumDataPoint {
                            attributes: attributes(),
                            value: m::MetricNumber::U64(3),
                            exemplars: vec![],
                        }],
                        start_time: time(),
                        time: time(),
                        temporality: m::Temporality::Cumulative,
                        is_monotonic: true,
                    }),
                },
                m::Metric {
                    name: "in-flight".into(),
                    description: String::new(),
                    unit: String::new(),
                    data: m::MetricData::S64Sum(m::Sum {
                        data_points: vec![m::SumDataPoint {
                            attributes: attributes(),
                            value: m::MetricNumber::S64(-2),
                            exemplars: vec![],
                        }],
                        start_time: time(),
                        time: time(),
                        temporality: m::Temporality::Cumulative,
                        is_monotonic: false,
                    }),
                },
            ],
        }],
    }
}

fn context(plugin: Option<Arc<WasiOtel>>) -> SharedCtx {
    #[cfg(feature = "wasi-tls")]
    crate::init_crypto();
    let mut builder = crate::engine::ctx::Ctx::builder("workload", "component");
    if let Some(plugin) = plugin {
        let plugin: Arc<dyn HostPlugin + Send + Sync> = plugin;
        builder = builder.with_plugins(std::collections::HashMap::from([(WASI_OTEL_ID, plugin)]));
    }
    SharedCtx::new(builder.build())
}

#[tokio::test]
async fn unconfigured_signals_are_safe_and_missing_metrics_are_reported() {
    for plugin in [None, Some(Arc::new(WasiOtel::default()))] {
        let present = plugin.is_some();
        let mut shared = context(plugin);
        let mut ctx = extract_active_ctx(&mut shared);
        for severity in [None, Some(9), Some(13), Some(17), Some(24)] {
            l::Host::on_emit(&mut ctx, record(severity)).await.unwrap();
        }
        t::Host::on_start(&mut ctx, span_context()).await.unwrap();
        t::Host::on_end(&mut ctx, span()).await.unwrap();
        assert_eq!(
            m::Host::export(&mut ctx, metrics()).await.unwrap().is_err(),
            present
        );
        let outer = t::Host::outer_span_context(&mut ctx).await.unwrap();
        assert!(outer.trace_id.is_empty());
    }
}

#[tokio::test]
async fn guest_telemetry_reaches_providers_and_stop_flushes_and_clears_them() {
    let spans = InMemorySpanExporter::default();
    let logs = InMemoryLogExporter::default();
    let metrics_exporter = InMemoryMetricExporter::default();
    let plugin = Arc::new(WasiOtel::default());
    *plugin.tracer_provider.write().await = Some(
        SdkTracerProvider::builder()
            .with_id_generator(GuestIdGenerator::default())
            .with_simple_exporter(spans.clone())
            .build(),
    );
    *plugin.logger_provider.write().await = Some(
        SdkLoggerProvider::builder()
            .with_simple_exporter(logs.clone())
            .build(),
    );
    *plugin.meter_provider.write().await = Some(
        SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(metrics_exporter.clone()).build())
            .build(),
    );
    let mut shared = context(Some(plugin.clone()));
    let mut ctx = extract_active_ctx(&mut shared);
    t::Host::on_end(&mut ctx, span()).await.unwrap();
    l::Host::on_emit(&mut ctx, record(Some(17))).await.unwrap();
    m::Host::export(&mut ctx, metrics()).await.unwrap().unwrap();
    let exported = spans.get_finished_spans().unwrap();
    assert_eq!(exported.len(), 1);
    let exported = &exported[0];
    assert_eq!(exported.name, "request");
    assert_eq!(
        exported.span_context.trace_id().to_string(),
        span_context().trace_id
    );
    assert_eq!(
        exported.span_context.span_id().to_string(),
        span_context().span_id
    );
    assert_eq!(exported.parent_span_id.to_string(), "fedcba9876543210");
    assert_eq!(exported.events.events.len(), 1);
    assert_eq!(exported.events.events[0].name, "ready");
    assert_eq!(logs.get_emitted_logs().unwrap().len(), 1);
    let exported = metrics_exporter.get_finished_metrics().unwrap();
    let mut names = exported
        .iter()
        .flat_map(|r| r.scope_metrics())
        .flat_map(|s| s.metrics())
        .map(|m| m.name())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["in-flight", "requests", "temperature"]);
    plugin.stop().await.unwrap();
    plugin.stop().await.unwrap();
    assert!(plugin.tracer_provider.read().await.is_none());
    assert!(plugin.logger_provider.read().await.is_none());
    assert!(plugin.meter_provider.read().await.is_none());
}

#[tokio::test]
async fn outer_context_preserves_the_callers_trace_identity() {
    use opentelemetry::trace::TraceContextExt;
    let mut shared = context(None);
    let mut ctx = extract_active_ctx(&mut shared);
    let expected = span_context();
    let context =
        opentelemetry::Context::new().with_remote_span_context(wit_span_context_to_otel(&expected));
    let _guard = context.attach();
    let actual = t::Host::outer_span_context(&mut ctx).await.unwrap();
    assert_eq!(actual.trace_id, expected.trace_id);
    assert_eq!(actual.span_id, expected.span_id);
}
