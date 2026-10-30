use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use bindings::wasi::{
    clocks::wall_clock::Datetime,
    otel::{logs as l, metrics as m, tracing as t, types},
};
use opentelemetry::logs::{AnyValue, Severity};
use opentelemetry::trace::{SpanId, SpanKind, Status};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::InMemoryLogExporter;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SpanData, SpanExporter};
use tracing_subscriber::layer::SubscriberExt as _;

const LINK_SPAN_ID: &str = "1111111111111111";

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

fn attributes() -> Vec<types::KeyValue> {
    vec![types::KeyValue {
        key: "route".into(),
        value: "\"/hello\"".into(),
    }]
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
    let mut linked = span_context();
    linked.span_id = LINK_SPAN_ID.into();
    t::SpanData {
        span_context: span_context(),
        parent_span_id: "fedcba9876543210".into(),
        span_kind: t::SpanKind::Server,
        name: "request".into(),
        start_time: time(),
        end_time: time(),
        attributes: attributes(),
        events: vec![t::Event {
            name: "ready".into(),
            time: time(),
            attributes: vec![],
        }],
        links: vec![t::Link {
            span_context: linked,
            attributes: vec![],
        }],
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
        attributes: Some(attributes()),
        event_name: Some("guest-log".into()),
        resource: None,
        instrumentation_scope: Some(scope()),
        trace_id: Some(span_context().trace_id),
        span_id: Some(span_context().span_id),
        trace_flags: Some(WitTraceFlags::SAMPLED),
    }
}

fn metrics() -> m::ResourceMetrics {
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

async fn plugin_tracing_to(exporter: impl SpanExporter + 'static) -> Arc<WasiOtel> {
    let plugin = Arc::new(WasiOtel::default());
    *plugin.tracer_provider.write().await = Some(
        SdkTracerProvider::builder()
            .with_id_generator(GuestIdGenerator::default())
            .with_simple_exporter(exporter)
            .build(),
    );
    plugin
}

/// Every f64 point by metric name: `None` for a gauge, `Some(is_monotonic)` for a sum.
fn f64_points(
    exported: &[ResourceMetrics],
) -> BTreeMap<String, (Option<bool>, f64, Vec<KeyValue>)> {
    let mut points = BTreeMap::new();
    for metric in exported
        .iter()
        .flat_map(|resource| resource.scope_metrics())
        .flat_map(|scope| scope.metrics())
    {
        let point = match metric.data() {
            AggregatedMetrics::F64(MetricData::Gauge(gauge)) => gauge
                .data_points()
                .map(|point| (None, point.value(), point.attributes().cloned().collect()))
                .next(),
            AggregatedMetrics::F64(MetricData::Sum(sum)) => sum
                .data_points()
                .map(|point| {
                    (
                        Some(sum.is_monotonic()),
                        point.value(),
                        point.attributes().cloned().collect(),
                    )
                })
                .next(),
            _ => panic!("unexpected aggregation for {}", metric.name()),
        };
        points.insert(metric.name().to_string(), point.expect("a data point"));
    }
    points
}

#[derive(Clone, Debug, Default)]
struct SpanNames(Arc<Mutex<Vec<String>>>);

impl SpanExporter for SpanNames {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let mut names = self.0.lock().unwrap();
        names.extend(batch.into_iter().map(|span| span.name.into_owned()));
        Ok(())
    }
}

#[derive(Clone, Default)]
struct Levels(Arc<Mutex<Vec<tracing::Level>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Levels {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let plugin_module = module_path!().trim_end_matches("::contract_tests");
        if event.metadata().module_path() == Some(plugin_module) {
            self.0.lock().unwrap().push(*event.metadata().level());
        }
    }
}

#[test]
fn the_plugin_declares_every_wasi_otel_interface() {
    let plugin = WasiOtel::default();
    assert_eq!(plugin.id(), WASI_OTEL_ID);
    let world = plugin.world();
    assert!(world.exports.is_empty());
    let imports = world.imports.into_iter().collect::<Vec<_>>();
    assert_eq!(imports.len(), 1);
    assert_eq!(
        (imports[0].namespace.as_str(), imports[0].package.as_str()),
        ("wasi", "otel")
    );
    let mut names = imports[0].interfaces.iter().cloned().collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["logs", "metrics", "tracing", "types"]);
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
        assert!(outer.span_id.is_empty());
        assert_eq!(outer.trace_flags, WitTraceFlags::empty());
        assert!(!outer.is_remote);
        assert!(outer.trace_state.is_empty());
    }
}

#[tokio::test]
async fn guest_log_severity_picks_the_host_log_level() {
    use tracing::Level;
    let levels = Levels::default();
    let _guard =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(levels.clone()));
    let mut shared = context(None);
    let mut ctx = extract_active_ctx(&mut shared);
    for severity in [
        None,
        Some(12),
        Some(13),
        Some(16),
        Some(17),
        Some(24),
        Some(25),
    ] {
        l::Host::on_emit(&mut ctx, record(severity)).await.unwrap();
    }
    assert_eq!(
        *levels.0.lock().unwrap(),
        [
            Level::DEBUG,
            Level::DEBUG,
            Level::WARN,
            Level::WARN,
            Level::ERROR,
            Level::ERROR,
            Level::DEBUG,
        ]
    );
}

#[tokio::test]
async fn guest_telemetry_reaches_providers_with_its_shape_intact() {
    let spans = InMemorySpanExporter::default();
    let logs = InMemoryLogExporter::default();
    let metrics_exporter = InMemoryMetricExporter::default();
    let plugin = plugin_tracing_to(spans.clone()).await;
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
    let service_name = plugin.config.service_name.clone();
    let mut shared = context(Some(plugin.clone()));
    let mut ctx = extract_active_ctx(&mut shared);
    t::Host::on_end(&mut ctx, span()).await.unwrap();
    l::Host::on_emit(&mut ctx, record(Some(17))).await.unwrap();
    m::Host::export(&mut ctx, metrics()).await.unwrap().unwrap();

    let exported = spans.get_finished_spans().unwrap();
    assert_eq!(exported.len(), 1);
    let exported = &exported[0];
    assert_eq!(exported.name, "request");
    assert_eq!(exported.instrumentation_scope.name(), service_name);
    assert_eq!(
        exported.span_context.trace_id().to_string(),
        span_context().trace_id
    );
    assert_eq!(
        exported.span_context.span_id().to_string(),
        span_context().span_id
    );
    assert_eq!(exported.parent_span_id.to_string(), "fedcba9876543210");
    assert_eq!(exported.span_kind, SpanKind::Server);
    assert_eq!(
        exported.attributes,
        vec![KeyValue::new("route", "\"/hello\"")]
    );
    assert_eq!(exported.status, Status::Ok);
    assert_eq!(exported.start_time, UNIX_EPOCH + Duration::new(10, 20));
    assert_eq!(exported.end_time, UNIX_EPOCH + Duration::new(10, 20));
    assert_eq!(exported.events.events.len(), 1);
    assert_eq!(exported.events.events[0].name, "ready");
    assert_eq!(exported.links.links.len(), 1);
    assert_eq!(
        exported.links.links[0].span_context.span_id().to_string(),
        LINK_SPAN_ID
    );

    let emitted = logs.get_emitted_logs().unwrap();
    assert_eq!(emitted.len(), 1);
    let log = &emitted[0];
    assert_eq!(log.instrumentation.name(), service_name);
    assert_eq!(log.record.severity_number(), Some(Severity::Error));
    assert_eq!(log.record.body(), Some(&AnyValue::String("hello".into())));
    let trace = log.record.trace_context().expect("trace context");
    assert_eq!(trace.trace_id.to_string(), span_context().trace_id);
    assert_eq!(trace.span_id.to_string(), span_context().span_id);
    let attributes = log
        .record
        .attributes_iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        attributes.get("route"),
        Some(&AnyValue::String("\"/hello\"".into()))
    );
    assert_eq!(
        attributes.get("resource.service.name"),
        Some(&AnyValue::String(service_name.clone().into()))
    );
    assert_eq!(
        attributes.get("instrumentation_scope.name"),
        Some(&AnyValue::String("guest".into()))
    );

    let route = vec![KeyValue::new("route", "\"/hello\"")];
    let points = f64_points(&metrics_exporter.get_finished_metrics().unwrap());
    assert_eq!(points["temperature"], (None, 1.5, route.clone()));
    assert_eq!(points["requests"], (Some(true), 3.0, route.clone()));
    assert_eq!(points["in-flight"], (Some(false), -2.0, route));

    plugin.stop().await.unwrap();
    plugin.stop().await.unwrap();
    assert!(plugin.tracer_provider.read().await.is_none());
    assert!(plugin.logger_provider.read().await.is_none());
    assert!(plugin.meter_provider.read().await.is_none());
}

#[tokio::test]
async fn stop_drains_batched_spans() {
    let names = SpanNames::default();
    let plugin = Arc::new(WasiOtel::default());
    *plugin.tracer_provider.write().await = Some(
        SdkTracerProvider::builder()
            .with_id_generator(GuestIdGenerator::default())
            .with_batch_exporter(names.clone())
            .build(),
    );
    let mut shared = context(Some(plugin.clone()));
    let mut ctx = extract_active_ctx(&mut shared);
    t::Host::on_end(&mut ctx, span()).await.unwrap();
    plugin.stop().await.unwrap();
    assert_eq!(*names.0.lock().unwrap(), ["request"]);
}

#[tokio::test]
async fn export_reports_a_flush_failure() {
    let plugin = Arc::new(WasiOtel::default());
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(InMemoryMetricExporter::default()).build())
        .build();
    provider.shutdown().unwrap();
    *plugin.meter_provider.write().await = Some(provider);
    let mut shared = context(Some(plugin));
    let mut ctx = extract_active_ctx(&mut shared);
    let error = m::Host::export(&mut ctx, metrics())
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.starts_with("Failed to flush metrics"), "{error}");
}

#[tokio::test]
async fn root_spans_have_no_parent_and_unsampled_spans_are_dropped() {
    let spans = InMemorySpanExporter::default();
    let plugin = plugin_tracing_to(spans.clone()).await;
    let mut shared = context(Some(plugin));
    let mut ctx = extract_active_ctx(&mut shared);

    let mut root = span();
    root.parent_span_id = "0000000000000000".into();
    t::Host::on_end(&mut ctx, root).await.unwrap();
    let exported = spans.get_finished_spans().unwrap();
    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].parent_span_id, SpanId::INVALID);
    assert_eq!(
        exported[0].span_context.span_id().to_string(),
        span_context().span_id
    );

    spans.reset();
    let mut unsampled = span();
    unsampled.span_context.trace_flags = WitTraceFlags::empty();
    t::Host::on_end(&mut ctx, unsampled).await.unwrap();
    assert!(spans.get_finished_spans().unwrap().is_empty());
}

#[tokio::test]
async fn outer_context_preserves_the_callers_trace_identity() {
    use opentelemetry::trace::TraceContextExt;
    let mut shared = context(None);
    let mut ctx = extract_active_ctx(&mut shared);
    let mut expected = span_context();
    expected.is_remote = true;
    expected.trace_state = vec![("vendor".into(), "state".into())];
    let context =
        opentelemetry::Context::new().with_remote_span_context(wit_span_context_to_otel(&expected));
    let _guard = context.attach();
    let actual = t::Host::outer_span_context(&mut ctx).await.unwrap();
    assert_eq!(actual.trace_id, expected.trace_id);
    assert_eq!(actual.span_id, expected.span_id);
    assert_eq!(actual.trace_flags, WitTraceFlags::SAMPLED);
    assert!(actual.is_remote);
    assert_eq!(actual.trace_state, expected.trace_state);
}

fn sums(
    temporality: m::Temporality,
    start_seconds: u64,
    requests: u64,
    in_flight: i64,
) -> m::ResourceMetrics {
    let sum =
        |name: &str, value: m::MetricNumber, is_monotonic: bool, data: fn(m::Sum) -> _| m::Metric {
            name: name.into(),
            description: String::new(),
            unit: String::new(),
            data: data(m::Sum {
                data_points: vec![m::SumDataPoint {
                    attributes: attributes(),
                    value,
                    exemplars: vec![],
                }],
                start_time: Datetime {
                    seconds: start_seconds,
                    nanoseconds: 0,
                },
                time: time(),
                temporality,
                is_monotonic,
            }),
        };
    m::ResourceMetrics {
        resource: types::Resource {
            attributes: vec![],
            schema_url: None,
        },
        scope_metrics: vec![m::ScopeMetrics {
            scope: scope(),
            metrics: vec![
                sum(
                    "requests",
                    m::MetricNumber::U64(requests),
                    true,
                    m::MetricData::U64Sum,
                ),
                sum(
                    "in-flight",
                    m::MetricNumber::S64(in_flight),
                    false,
                    m::MetricData::S64Sum,
                ),
            ],
        }],
    }
}

/// A plugin exporting to the returned in-memory exporter.
async fn metering_plugin() -> (Arc<WasiOtel>, InMemoryMetricExporter) {
    let exporter = InMemoryMetricExporter::default();
    let plugin = Arc::new(WasiOtel::default());
    *plugin.meter_provider.write().await = Some(
        SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build(),
    );
    (plugin, exporter)
}

/// The instruments' `(requests, in-flight)` totals after the latest export.
fn latest_totals(exporter: &InMemoryMetricExporter) -> (f64, f64) {
    let finished = exporter.get_finished_metrics().unwrap();
    let points = f64_points(&finished[finished.len() - 1..]);
    (points["requests"].1, points["in-flight"].1)
}

/// The instruments' totals after each export in `exports`, in order, all from
/// one guest instance.
async fn totals_after(exports: Vec<m::ResourceMetrics>) -> Vec<(f64, f64)> {
    let (plugin, exporter) = metering_plugin().await;
    let mut shared = context(Some(plugin));
    let mut ctx = extract_active_ctx(&mut shared);
    let mut totals = Vec::new();
    for export in exports {
        m::Host::export(&mut ctx, export).await.unwrap().unwrap();
        totals.push(latest_totals(&exporter));
    }
    totals
}

#[tokio::test]
async fn repeated_cumulative_exports_add_only_their_growth() {
    use m::Temporality::Cumulative;
    let totals = totals_after(vec![
        sums(Cumulative, 1, 3, -2),
        sums(Cumulative, 1, 3, -2),
        sums(Cumulative, 1, 5, -1),
    ])
    .await;
    assert_eq!(totals, [(3.0, -2.0), (3.0, -2.0), (5.0, -1.0)]);
}

#[tokio::test]
async fn instances_sharing_a_start_time_count_separately() {
    use m::Temporality::Cumulative;
    let (plugin, exporter) = metering_plugin().await;
    let mut instances = [context(Some(plugin.clone())), context(Some(plugin))];
    let mut totals = Vec::new();
    for (instance, requests) in [(0, 3), (1, 2), (0, 4)] {
        let mut ctx = extract_active_ctx(&mut instances[instance]);
        m::Host::export(&mut ctx, sums(Cumulative, 1, requests, 1))
            .await
            .unwrap()
            .unwrap();
        totals.push(latest_totals(&exporter));
    }
    assert_eq!(totals, [(3.0, 1.0), (5.0, 2.0), (6.0, 2.0)]);
}

#[tokio::test]
async fn a_new_start_time_begins_a_new_total() {
    use m::Temporality::Cumulative;
    let totals = totals_after(vec![
        sums(Cumulative, 1, 3, 1),
        sums(Cumulative, 2, 1, 1),
        sums(Cumulative, 2, 2, 1),
    ])
    .await;
    assert_eq!(totals, [(3.0, 1.0), (4.0, 2.0), (5.0, 2.0)]);
}

#[tokio::test]
async fn a_monotonic_total_that_falls_restarted_from_zero() {
    use m::Temporality::Cumulative;
    let totals = totals_after(vec![sums(Cumulative, 1, 5, 0), sums(Cumulative, 1, 2, 0)]).await;
    assert_eq!(totals, [(5.0, 0.0), (7.0, 0.0)]);
}

#[tokio::test]
async fn delta_exports_add_every_value() {
    let totals = totals_after(vec![
        sums(m::Temporality::Delta, 1, 3, -2),
        sums(m::Temporality::Delta, 1, 3, -2),
    ])
    .await;
    assert_eq!(totals, [(3.0, -2.0), (6.0, -4.0)]);
}

#[tokio::test]
async fn low_memory_counters_are_deltas_and_up_down_counters_are_totals() {
    let totals = totals_after(vec![
        sums(m::Temporality::LowMemory, 1, 3, -2),
        sums(m::Temporality::LowMemory, 1, 3, -1),
    ])
    .await;
    assert_eq!(totals, [(3.0, -2.0), (6.0, -1.0)]);
}

#[test]
fn an_instance_stops_tracking_new_series_at_the_limit() {
    let counter = |name: String, value| convert::CounterValue {
        scope: "scope".into(),
        name,
        value,
        is_monotonic: true,
        attributes: vec![],
        series_start: Some((1, 0)),
    };
    let mut totals = CumulativeTotals::default();
    for index in 0..MAX_CUMULATIVE_SERIES {
        assert_eq!(
            totals.increment(&counter(format!("m{index}"), 1.0)),
            Some(1.0)
        );
    }
    assert_eq!(totals.increment(&counter("late".into(), 1.0)), None);
    assert_eq!(totals.increment(&counter("late".into(), 2.0)), None);
    assert_eq!(totals.increment(&counter("m0".into(), 3.0)), Some(2.0));
}
