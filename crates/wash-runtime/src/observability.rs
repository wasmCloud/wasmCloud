use std::{any::Any, collections::HashMap, sync::Arc};

use anyhow::Context;

use opentelemetry::{KeyValue, trace::TracerProvider};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::Resource;
use opentelemetry_semantic_conventions::resource;
use tracing::Level;
use tracing_subscriber::{
    EnvFilter, Layer, Registry, filter::Directive, layer::SubscriberExt, util::SubscriberInitExt,
};

/// Initialize observability, setting up console & OpenTelemetry layers.
///
/// Returns a shutdown function that should be called on process exit to flush any remaining spans/logs
pub fn initialize_observability(
    log_level: Level,
    ansi_colors: bool,
    verbose: bool,
) -> anyhow::Result<Box<dyn FnOnce()>> {
    // STDERR logging layer
    let mut fmt_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level.as_str()));
    if !verbose {
        // async_nats prints out on connect
        fmt_filter = fmt_filter
            .add_directive(directive("async_nats=error")?)
            // wasm_pkg_client/core are a little verbose so we set them to error level in non-verbose mode
            .add_directive(directive("wasm_pkg_client=error")?)
            .add_directive(directive("wasm_pkg_core=error")?);
    }

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_level(true)
        .with_target(verbose)
        .with_thread_ids(verbose)
        .with_thread_names(verbose)
        .with_file(verbose)
        .with_line_number(verbose)
        .with_ansi(ansi_colors)
        .with_filter(fmt_filter);

    let otel_enabled = std::env::vars().any(|(key, _)| key.starts_with("OTEL_"));
    if !otel_enabled {
        Registry::default().with(fmt_layer).init();

        // No-op shutdown function
        let shutdown_fn = || {};
        return Ok(Box::new(shutdown_fn));
    }

    let resource = Resource::builder()
        .with_attribute(KeyValue::new(
            resource::SERVICE_NAME.to_string(),
            env!("CARGO_PKG_NAME"),
        ))
        .with_attribute(KeyValue::new(
            resource::SERVICE_INSTANCE_ID.to_string(),
            uuid::Uuid::new_v4().to_string(),
        ))
        .with_attribute(KeyValue::new(
            resource::SERVICE_VERSION.to_string(),
            env!("CARGO_PKG_VERSION"),
        ))
        .build();

    // OTel logging layer
    let log_exporter = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .build()?;
    let log_provider = opentelemetry_sdk::logs::LoggerProviderBuilder::default()
        .with_batch_exporter(log_exporter)
        .with_resource(resource.clone())
        .build();
    let filter_otel_logs = EnvFilter::new(log_level.as_str());

    let otel_logs_layer =
        OpenTelemetryTracingBridge::new(&log_provider).with_filter(filter_otel_logs);

    // OTel tracing layer
    let tracer_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()?;
    let tracer_provider = opentelemetry_sdk::trace::TracerProviderBuilder::default()
        .with_batch_exporter(tracer_exporter)
        .with_resource(resource.clone())
        .build();

    let filter_otel_traces = EnvFilter::new(log_level.as_str());

    let otel_tracer_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer_provider.tracer("runtime"))
        .with_error_records_to_exceptions(true)
        .with_error_fields_to_exceptions(true)
        .with_error_events_to_status(true)
        .with_error_events_to_exceptions(true)
        .with_location(true)
        .with_filter(filter_otel_traces);

    Registry::default()
        .with(fmt_layer)
        .with(otel_logs_layer)
        .with(otel_tracer_layer)
        .init();

    let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .build()
        .context("failed to create OTEL tonic exporter")?;

    let meter_provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
        .with_periodic_exporter(metric_exporter)
        .with_resource(resource.clone())
        .build();

    opentelemetry::global::set_meter_provider(meter_provider.clone());

    // Return a shutdown function to flush providers on exit
    let shutdown_fn = move || {
        if let Err(e) = tracer_provider.shutdown() {
            eprintln!("failed to shutdown tracer provider: {e}");
        }
        if let Err(e) = log_provider.shutdown() {
            eprintln!("failed to shutdown log provider: {e}");
        }
        if let Err(e) = meter_provider.shutdown() {
            eprintln!("failed to shutdown meter provider: {e}");
        }
    };

    Ok(Box::new(shutdown_fn))
}

/// Helper function to reduce duplication and code size for parsing directives
fn directive(directive: impl AsRef<str>) -> anyhow::Result<Directive> {
    directive
        .as_ref()
        .parse()
        .with_context(|| format!("failed to parse filter: {}", directive.as_ref()))
}

#[derive(Clone, Default)]
pub struct Meters {
    pub fuel_consumption: FuelConsumptionMeter,
    /// User-defined meters
    pub meters: HashMap<String, Arc<dyn Any + Send + Sync + 'static>>,
}

impl Meters {
    pub fn new(enabled: bool) -> Self {
        Self {
            fuel_consumption: FuelConsumptionMeter::new(enabled),
            meters: Default::default(),
        }
    }
}

#[derive(Clone, Default)]
pub struct FuelConsumptionMeter {
    hist: Option<opentelemetry::metrics::Histogram<u64>>,
}

impl FuelConsumptionMeter {
    pub(crate) fn new(enabled: bool) -> Self {
        let hist = enabled.then(|| {
            opentelemetry::global::meter("wash-runtime")
                .u64_histogram("fuel.consumption")
                .with_description(
                    "Measure fuel consumption for components that export host plugin interfaces",
                )
                .with_boundaries(fuel_histogram_boundaries())
                .build()
        });
        Self { hist }
    }

    pub async fn observe<T, F, R>(
        &self,
        attributes: &[KeyValue],
        store: &mut wasmtime::Store<T>,
        func: F,
    ) -> anyhow::Result<R>
    where
        F: AsyncFnOnce(&mut wasmtime::Store<T>) -> anyhow::Result<R>,
    {
        if let Some(fuel_meter) = &self.hist {
            store.set_fuel(u64::MAX)?;
            let result = func(store).await?;
            let consumed_fuel = u64::MAX - store.get_fuel()?;
            fuel_meter.record(consumed_fuel, attributes);

            Ok(result)
        } else {
            func(store).await
        }
    }
}

/// Generate histogram boundaries for fuel consumption metrics.
///
/// Produces boundaries following multipliers [1, 2.5, 5, 7.5] per decade,
/// starting at 50,000 up to a u64::MAX
fn fuel_histogram_boundaries() -> Vec<f64> {
    const MAX: f64 = u64::MAX as f64;
    const MULTIPLIERS: [f64; 4] = [1.0, 2.5, 5.0, 7.5];

    let mut boundaries = vec![0.0];
    let mut base = 50_000.0;
    loop {
        for &m in &MULTIPLIERS {
            let value = base * m;
            if value > MAX {
                return boundaries;
            }
            boundaries.push(value);
        }
        base *= 10.0;
    }
}
