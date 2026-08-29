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

    // Register the W3C Trace Context propagator so the incoming-request path
    // (`opentelemetry::global::get_text_map_propagator` in `host::http`) can
    // parse the `traceparent` header into the OpenTelemetry context.
    // Without this every workload roots its own trace instead of continuing the
    // caller's. Registering it here is what lets a trace roll up across
    // workload/host boundaries.
    opentelemetry::global::set_text_map_propagator(
        opentelemetry_sdk::propagation::TraceContextPropagator::new(),
    );

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

/// How a host measures the time its guests spend running.
///
/// The two meters answer the same question and cost very differently, so a host
/// picks one rather than paying for both:
///
/// * [`Self::Epoch`] samples the epoch callback every store arms anyway, so it
///   costs the guest nothing at run time. It reports whole milliseconds and is
///   a floor, not a total — see [`crate::engine::abandon::GuestExecution`].
/// * [`Self::Fuel`] counts executed operations exactly, at the price of a
///   counter compiled into every block of guest code, which the guest pays
///   whether or not anyone reads the number.
///
/// [`Self::Fuel`] is also the only one that needs anything of the engine:
/// `Config::consume_fuel` has to be on, and a store on such an engine starts
/// with no fuel, so guests only run because every store is given a budget.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MeterKind {
    /// Measure neither. The default: a host that was not asked to measure
    /// guest execution should not make its guests slower.
    #[default]
    Off,
    /// `guest.execution.time`, sampled from the epoch callback.
    Epoch,
    /// `fuel.consumption`, counted in the guest.
    Fuel,
}

impl MeterKind {
    /// Whether the engine has to compile fuel counters into its guests.
    pub fn consumes_fuel(&self) -> bool {
        matches!(self, Self::Fuel)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Epoch => "epoch",
            Self::Fuel => "fuel",
        }
    }
}

impl std::str::FromStr for MeterKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "off" | "none" => Ok(Self::Off),
            "epoch" => Ok(Self::Epoch),
            "fuel" => Ok(Self::Fuel),
            other => Err(format!(
                "unknown meter `{other}`, expected one of: off, epoch, fuel"
            )),
        }
    }
}

#[derive(Clone, Default)]
pub struct Meters {
    /// Built only under [`MeterKind::Fuel`]; inert otherwise, so a call path
    /// that measures with it records nothing rather than having to ask which
    /// meter the host chose.
    pub fuel_consumption: FuelConsumptionMeter,
    /// Built only under [`MeterKind::Epoch`], and inert otherwise for the same
    /// reason.
    pub execution_time: ExecutionTimeMeter,
    /// User-defined meters
    pub meters: HashMap<String, Arc<dyn Any + Send + Sync + 'static>>,
}

/// The execution-time histogram, reachable without a `Meters` in hand.
///
/// A pooled call runs inside the driver's `run_concurrent`, several layers
/// below anything holding a `Meters`, and carrying one down to every driver
/// would thread a metric through the engine's whole dispatch path. The
/// histogram is a handle rather than host state — OTel's own meter registry is
/// process-global for the same reason — so it is published here once and read
/// where a call actually ends.
static EXECUTION_TIME: std::sync::OnceLock<ExecutionTimeMeter> = std::sync::OnceLock::new();

/// The execution-time meter, once a host has built its [`Meters`].
///
/// `None` before that, and on a host that did not enable metering the meter
/// itself is inert, so a caller never has to ask which.
pub fn execution_time_meter() -> Option<&'static ExecutionTimeMeter> {
    EXECUTION_TIME.get()
}

impl Meters {
    pub fn new(kind: MeterKind) -> Self {
        let meters = Self {
            fuel_consumption: FuelConsumptionMeter::new(kind == MeterKind::Fuel),
            execution_time: ExecutionTimeMeter::new(kind == MeterKind::Epoch),
            meters: Default::default(),
        };
        // First host with metering on wins. A second one in the same process
        // (tests, an embedder running two) records into the first's histogram,
        // which is the same instrument OTel would have handed it anyway.
        // Skipping the disabled ones matters: a host built with metering off
        // would otherwise claim the slot and silence every pooled call after
        // it, including calls on a later host that did ask for metrics.
        if kind == MeterKind::Epoch {
            let _ = EXECUTION_TIME.set(meters.execution_time.clone());
        }
        meters
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

/// Reports how long a call ran guest code, in milliseconds.
///
/// The alternative to [`FuelConsumptionMeter`], and what the `wasmcloud:nats`
/// plugin measures its handlers with. Sampled by the epoch callback every store
/// arms anyway ([`crate::engine::abandon::GuestExecution`]), so it costs the
/// guest nothing at run time, where fuel's per-block counters are compiled into
/// the guest and slow it down whether or not anyone is watching. Fuel stays the
/// default everywhere it already was.
///
/// The trade is resolution, and it only ever undercounts: a call whose guest
/// runs for less than one sampling window is never sampled and records **0**.
/// Read the histogram as "calls that burned at least a window of guest
/// execution", not as a CPU total — a host serving nothing but short calls
/// reports zero and is not idle. `GuestExecution`'s docs have the rest,
/// including why a store serving concurrent calls attributes their execution
/// to each other.
#[derive(Clone, Default)]
pub struct ExecutionTimeMeter {
    hist: Option<opentelemetry::metrics::Histogram<u64>>,
}

impl ExecutionTimeMeter {
    pub(crate) fn new(enabled: bool) -> Self {
        let hist = enabled.then(|| {
            opentelemetry::global::meter("wash-runtime")
                .u64_histogram("guest.execution.time")
                .with_description(
                    "Guest execution time for components that export host plugin interfaces, \
                     sampled from the engine's epoch callback",
                )
                .with_unit("ms")
                .with_boundaries(execution_time_histogram_boundaries())
                .build()
        });
        Self { hist }
    }

    /// Records the guest execution one call added, measured by the caller.
    ///
    /// The store-taking [`Self::observe`] cannot be used from inside a pooled
    /// call: the driver owns the store for the instance's whole life and calls
    /// arrive as tasks on it, so nobody down there has a `&mut Store`. Read
    /// `SharedCtx::executed` through the accessor either side of the call and
    /// hand the delta here instead. Same counter, same caveats — read
    /// [`crate::engine::abandon::GuestExecution`] before reading the number,
    /// and note that on an instance serving several calls at once the delta
    /// includes its neighbours'.
    pub fn record(&self, attributes: &[KeyValue], millis: u64) {
        if let Some(hist) = &self.hist {
            hist.record(millis, attributes);
        }
    }

    pub async fn observe<F, R>(
        &self,
        attributes: &[KeyValue],
        store: &mut wasmtime::Store<crate::engine::ctx::SharedCtx>,
        func: F,
    ) -> anyhow::Result<R>
    where
        F: AsyncFnOnce(&mut wasmtime::Store<crate::engine::ctx::SharedCtx>) -> anyhow::Result<R>,
    {
        let Some(hist) = &self.hist else {
            return func(store).await;
        };
        // Cloned out rather than re-read after the call: `func` takes the
        // store, and the counter is shared with the callback either way.
        let executed = Arc::clone(&store.data().executed);
        let before = executed.millis();
        // Recorded before the `?`: a call that traps or errors still consumed
        // the time it ran for, and dropping those samples biases the histogram
        // toward the calls that succeeded.
        let result = func(store).await;
        hist.record(executed.millis().saturating_sub(before), attributes);
        result
    }
}

/// Generate histogram boundaries for guest execution time, in milliseconds.
///
/// Produces boundaries following multipliers [1, 2.5, 5, 7.5] per decade,
/// starting at one sampling window — below which every call records zero, so
/// finer buckets would only split the zero bucket — up to an hour.
fn execution_time_histogram_boundaries() -> Vec<f64> {
    const MAX: f64 = 3_600_000.0;
    const MULTIPLIERS: [f64; 4] = [1.0, 2.5, 5.0, 7.5];

    let mut boundaries = vec![0.0];
    // Floored at 1ms: a sub-millisecond window truncates to zero, and a zero
    // base stays zero through `base *= 10.0`, so the loop would never reach
    // `MAX` and would push boundaries until it ran out of memory.
    let mut base = (crate::engine::abandon::sampling_window().as_millis() as f64).max(1.0);
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
