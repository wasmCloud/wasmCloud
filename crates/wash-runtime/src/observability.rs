use std::{any::Any, collections::HashMap, sync::Arc};

use anyhow::Context;

use std::time::Duration;

use opentelemetry::{KeyValue, trace::TracerProvider};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::Resource;
use opentelemetry_semantic_conventions::attribute::ERROR_TYPE;
use opentelemetry_semantic_conventions::resource;
use tracing::Level;
use tracing_subscriber::{
    EnvFilter, Layer, Registry, filter::Directive, layer::SubscriberExt, util::SubscriberInitExt,
};

/// Flushes the OTel exporters, if [`initialize_observability`] installed any.
///
/// **Blocks** for up to five seconds per provider — the SDK's own timeout — so
/// a signal path has to bound it and keep it off the runtime the exporter
/// drains over. Runs at most once; a no-op when no exporter was installed.
pub fn flush() {
    static FLUSHED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    // Not a `Once`: `call_once` poisons, so one exporter panicking here would
    // turn every later flush — including the one `main` makes — into a panic.
    if FLUSHED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    if let Some(shutdown) = SHUTDOWN.get() {
        shutdown();
    }
}

/// Set once by [`initialize_observability`], so [`flush`] can reach the
/// providers it built without every exit path having to be handed them.
static SHUTDOWN: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

/// Initialize observability, setting up console & OpenTelemetry layers.
///
/// Returns a shutdown function that should be called on process exit to flush
/// any remaining spans/logs. It is [`flush`], which runs at most once — so a
/// process that already flushed on its way out of a signal handler does not
/// shut the providers down twice.
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

        // Nothing to flush: `flush` finds no registered shutdown and returns.
        return Ok(Box::new(flush));
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

    // Registered rather than only returned: every way this process can end has
    // to be able to flush, and `main` is not on all of them.
    let _ = SHUTDOWN.set(Box::new(move || {
        if let Err(e) = tracer_provider.shutdown() {
            eprintln!("failed to shutdown tracer provider: {e}");
        }
        if let Err(e) = log_provider.shutdown() {
            eprintln!("failed to shutdown log provider: {e}");
        }
        if let Err(e) = meter_provider.shutdown() {
            eprintln!("failed to shutdown meter provider: {e}");
        }
    }));

    Ok(Box::new(flush))
}

/// Helper function to reduce duplication and code size for parsing directives
fn directive(directive: impl AsRef<str>) -> anyhow::Result<Directive> {
    directive
        .as_ref()
        .parse()
        .with_context(|| format!("failed to parse filter: {}", directive.as_ref()))
}

/// What a host measures about its guests.
///
/// Rate, errors and duration ([`InvocationMeter`]) are what an operator asks
/// for first and cost two clock reads, so they are what `Duration` — the
/// default — gives. `Fuel` adds an exact count of the operations a guest
/// executed, which is the only accurate guest-work signal this runtime has and
/// is not free: `Config::consume_fuel` compiles a counter into every block of
/// guest code, and the guest pays it whether or not anyone reads the number.
///
/// There is no epoch option. Epoch sampling looked free, and was, but it
/// reports a *store's* execution only when the store runs a full sampling
/// window without a call starting — and `rearm_for_call` restarts that window
/// on every call. A store taking calls more often than once per window credits
/// nothing at all, so the signal fell to zero exactly as a host got busy. See
/// [`crate::engine::abandon::GuestExecution`], which still counts for the
/// teardown it was built for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MeterKind {
    /// Measure nothing.
    Off,
    /// `guest.invocation.duration`: rate, errors and duration.
    #[default]
    Duration,
    /// The above, plus `fuel.consumption`.
    Fuel,
}

impl MeterKind {
    /// Whether the engine has to compile fuel counters into its guests.
    pub fn consumes_fuel(&self) -> bool {
        matches!(self, Self::Fuel)
    }

    /// Whether anything is measured at all.
    pub fn records(&self) -> bool {
        !matches!(self, Self::Off)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Duration => "duration",
            Self::Fuel => "fuel",
        }
    }
}

impl std::str::FromStr for MeterKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "off" | "none" => Ok(Self::Off),
            "duration" => Ok(Self::Duration),
            "fuel" => Ok(Self::Fuel),
            other => Err(format!(
                "unknown meter `{other}`, expected one of: off, duration, fuel"
            )),
        }
    }
}

#[derive(Clone)]
pub struct Meters {
    /// Rate, errors and duration. Built for any [`MeterKind`] but `Off`: it
    /// costs the guest two clock reads, and it is the one an operator needs
    /// first.
    pub invocation: InvocationMeter,
    /// Built only under [`MeterKind::Fuel`]; inert otherwise, so a call path
    /// that measures with it records nothing rather than having to ask which
    /// meter the host chose.
    pub fuel_consumption: FuelConsumptionMeter,
    /// User-defined meters
    pub meters: HashMap<String, Arc<dyn Any + Send + Sync + 'static>>,
}

/// Who ran the guest code a measurement covers, in the terms a manifest author
/// and a dashboard both recognize.
///
/// Every guest-execution measurement carries these three, whichever plugin
/// drove the call and whichever of the two histograms records it, so one query
/// groups calls across all of them. See [`Self::attributes`] for the full
/// scheme.
///
/// Deliberately **not** the workload or component id: both are
/// `uuid::Uuid::new_v4()` minted per workload construction, so attributing a
/// series with one mints a fresh series on every restart, rolling update and
/// replica — growth driven by deployment churn, and a value no operator can map
/// back to a workload. The ids keep their place on the span and the log line,
/// where identity is per-event and therefore free. This is the same rule
/// `wasmcloud:messaging`'s admission counter follows.
#[derive(Clone, Debug)]
pub struct WorkloadIdentity {
    /// The workload's manifest namespace.
    pub namespace: Arc<str>,
    /// The workload's manifest name.
    pub name: Arc<str>,
    /// The component's *manifest* name, so a workload running more than one
    /// component can still be told apart.
    pub component: Arc<str>,
}

impl WorkloadIdentity {
    pub fn new(namespace: &str, name: &str, component: &str) -> Self {
        Self {
            namespace: Arc::from(namespace),
            name: Arc::from(name),
            component: Arc::from(component),
        }
    }

    /// The attribute set shared by every guest-execution measurement.
    ///
    /// `plugin` names the host surface that drove the call, bounded by what is
    /// compiled in. `operation` names the WIT export invoked — for example
    /// `wasmcloud:nats/core-handler#handle-message` — bounded by the interface
    /// set a component declares. Neither can be invented by traffic, so the
    /// series count stays bounded by what is deployed.
    ///
    /// A call surface with a further *bounded* dimension appends its own
    /// through [`Self::attributes_with`]. Anything a caller can invent belongs
    /// on the span instead, where it costs one event rather than one time
    /// series per histogram bucket, forever.
    ///
    /// Shared rather than owned: every value here is fixed for the life of a
    /// subscription or a route, so callers build one set when that is set up
    /// and clone the handle per call. Rebuilding it per call would allocate the
    /// vector and five strings on a delivery hot path to arrive at the same
    /// answer every time.
    pub fn attributes(&self, plugin: &'static str, operation: &str) -> Arc<[KeyValue]> {
        self.build(plugin, operation, None)
    }

    /// As [`Self::attributes`], plus one further dimension this surface bounds
    /// itself — an HTTP method, a configured subscription.
    pub fn attributes_with(
        &self,
        plugin: &'static str,
        operation: &str,
        extra: KeyValue,
    ) -> Arc<[KeyValue]> {
        self.build(plugin, operation, Some(extra))
    }

    /// The identity trio both instruments carry. One definition, so a rename
    /// cannot leave `guest.execution.time` and `guest.execution.total`
    /// disagreeing about the key to join on.
    fn identity_keys(&self) -> [KeyValue; 3] {
        [
            KeyValue::new("workload.namespace", self.namespace.to_string()),
            KeyValue::new("workload.name", self.name.to_string()),
            KeyValue::new("component", self.component.to_string()),
        ]
    }

    fn build(
        &self,
        plugin: &'static str,
        operation: &str,
        extra: Option<KeyValue>,
    ) -> Arc<[KeyValue]> {
        let mut attributes = Vec::with_capacity(5 + usize::from(extra.is_some()));
        attributes.push(KeyValue::new("plugin", plugin));
        attributes.push(KeyValue::new("operation", operation.to_string()));
        attributes.extend(self.identity_keys());
        attributes.extend(extra);
        attributes.into()
    }
}

/// Rate, errors and duration for one guest invocation — the three questions
/// asked of a serverless platform, from one instrument.
///
/// Wall clock, not guest execution: it is what a caller waited, it is exact per
/// call, and it costs two `Instant::now()`. `guest.execution.total` answers the
/// different question of how much CPU a workload burned, and cannot answer this
/// one — the epoch sampler credits a *store*, and only in whole sampling
/// windows.
///
/// One histogram rather than three instruments, which is what the OTel
/// conventions do: its count is the rate, and `error.type` — present only on a
/// failure, per the convention — separates the errors out of the same series.
#[derive(Clone, Default)]
pub struct InvocationMeter {
    duration: Option<opentelemetry::metrics::Histogram<f64>>,
}

impl InvocationMeter {
    pub(crate) fn new(enabled: bool) -> Self {
        let duration = enabled.then(|| {
            opentelemetry::global::meter("wash-runtime")
                .f64_histogram("guest.invocation.duration")
                .with_description("Wall-clock duration of one guest invocation")
                .with_unit("s")
                .with_boundaries(vec![
                    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
                ])
                .build()
        });
        Self { duration }
    }

    /// Record one invocation. `error` names what went wrong, and is absent when
    /// nothing did — the convention's own way of marking a failure, so an error
    /// rate is a filter on this series rather than a second instrument to keep
    /// in step with it.
    /// Whether this meter records anything.
    pub(crate) fn is_enabled(&self) -> bool {
        self.duration.is_some()
    }

    pub fn record(&self, attributes: &[KeyValue], elapsed: Duration, error: Option<&'static str>) {
        let Some(duration) = &self.duration else {
            return;
        };
        match error {
            None => duration.record(elapsed.as_secs_f64(), attributes),
            Some(error) => {
                let mut with_error = attributes.to_vec();
                with_error.push(KeyValue::new(ERROR_TYPE, error));
                duration.record(elapsed.as_secs_f64(), &with_error);
            }
        }
    }
}

/// [`MeterKind::default()`], so both doors into a host — the enum and
/// [`crate::host::HostBuilder`] — agree on what a host nobody configured
/// measures.
///
/// Written out rather than derived: field-by-field, every meter's own default
/// is the inert one, which is `Off` under a type whose default is `Duration`.
impl Default for Meters {
    fn default() -> Self {
        Self::new(MeterKind::default())
    }
}

impl Meters {
    /// The meter a call path measures through; see [`GuestMeter`].
    pub fn guest(&self) -> GuestMeter {
        GuestMeter {
            fuel: self.fuel_consumption.clone(),
            invocation: self.invocation.clone(),
        }
    }

    pub fn new(kind: MeterKind) -> Self {
        Self {
            invocation: InvocationMeter::new(kind.records()),
            fuel_consumption: FuelConsumptionMeter::new(kind.consumes_fuel()),
            meters: Default::default(),
        }
    }
}

#[derive(Clone, Default)]
pub struct FuelConsumptionMeter {
    hist: Option<opentelemetry::metrics::Histogram<u64>>,
}

/// The guest-execution meter a host runs, whichever kind it chose.
///
/// A call path that can hand over its `&mut Store` measures through this rather
/// than naming one of the two meters, so [`MeterKind`] decides *how* the
/// measurement is taken and the path does not have to know.
///
/// Fuel is the only kind that has to wrap the call: it is read off the store
/// either side of it. Duration is timed here for every kind, so a path that
/// measures through this gets rate, errors and duration whichever kind the
/// host chose.
#[derive(Clone, Default)]
pub struct GuestMeter {
    fuel: FuelConsumptionMeter,
    /// Carried rather than read from the global: this is built from the
    /// [`Meters`] a host owns, so it can use that host's instrument instead of
    /// whichever one happened to publish itself first.
    invocation: InvocationMeter,
}

impl GuestMeter {
    /// Measure one call: its duration always, and its fuel when the host asked
    /// for fuel.
    ///
    /// Falls through to the call itself when nothing is metering, so a path
    /// never has to ask whether metering is enabled.
    pub async fn observe<F, R>(
        &self,
        attributes: &[KeyValue],
        store: &mut wasmtime::Store<crate::engine::ctx::SharedCtx>,
        func: F,
    ) -> anyhow::Result<R>
    where
        F: AsyncFnOnce(&mut wasmtime::Store<crate::engine::ctx::SharedCtx>) -> anyhow::Result<R>,
    {
        if !self.invocation.is_enabled() {
            return if self.fuel.is_enabled() {
                self.fuel.observe(attributes, store, func).await
            } else {
                func(store).await
            };
        }
        let started = std::time::Instant::now();
        let result = if self.fuel.is_enabled() {
            self.fuel.observe(attributes, store, func).await
        } else {
            func(store).await
        };
        // A host failure is all this layer can see; a guest that ran and
        // returned its own error is a success here and named by whoever reads
        // that error.
        let error = result.is_err().then_some("host");
        self.invocation.record(attributes, started.elapsed(), error);
        result
    }
}

impl FuelConsumptionMeter {
    /// Whether this meter records anything, which is how [`GuestMeter`] picks
    /// between the two without consulting the [`MeterKind`] again.
    pub(crate) fn is_enabled(&self) -> bool {
        self.hist.is_some()
    }

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
        // `set_fuel` errors on an engine built without `Config::consume_fuel`,
        // and a call the caller asked for must not fail over a number nobody
        // can read. `HostBuilder` warns about the mismatch; here it just costs
        // the measurement.
        let Some(fuel_meter) = &self.hist else {
            return func(store).await;
        };
        if store.set_fuel(u64::MAX).is_err() {
            return func(store).await;
        }
        let result = func(store).await?;
        let consumed_fuel = u64::MAX - store.get_fuel()?;
        fuel_meter.record(consumed_fuel, attributes);

        Ok(result)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_and_values(attributes: &[KeyValue]) -> Vec<(String, String)> {
        attributes
            .iter()
            .map(|kv| (kv.key.to_string(), kv.value.to_string()))
            .collect()
    }

    /// The scheme every guest-execution measurement shares, pinned by key.
    ///
    /// A rename here silently splits a dashboard's series in two, and nothing
    /// else in the crate asserts these names.
    #[test]
    fn the_attribute_set_is_the_documented_scheme() {
        let identity = WorkloadIdentity::new("shop", "checkout", "api");
        assert_eq!(
            keys_and_values(&identity.attributes("wasi-http", "wasi:http/handler#handle")),
            vec![
                ("plugin".to_string(), "wasi-http".to_string()),
                (
                    "operation".to_string(),
                    "wasi:http/handler#handle".to_string()
                ),
                ("workload.namespace".to_string(), "shop".to_string()),
                ("workload.name".to_string(), "checkout".to_string()),
                ("component".to_string(), "api".to_string()),
            ]
        );
    }

    /// A surface's own bounded dimension appends, and does not displace.
    ///
    /// Spelled the way HTTP spells it: the semconv key, so the histogram and
    /// the spans that already carry `http.request.method` join on it.
    #[test]
    fn an_extra_dimension_appends_to_the_shared_set() {
        let identity = WorkloadIdentity::new("shop", "checkout", "api");
        let attributes = identity.attributes_with(
            "wasi-http",
            "wasi:http/handler#handle",
            KeyValue::new(
                opentelemetry_semantic_conventions::attribute::HTTP_REQUEST_METHOD,
                "GET",
            ),
        );
        assert_eq!(attributes.len(), 6);
        assert_eq!(
            keys_and_values(&attributes).last().cloned(),
            Some(("http.request.method".to_string(), "GET".to_string()))
        );
    }

    /// Only the meter that was chosen is built. The other stays inert rather
    /// than recording into a histogram nobody asked for.
    #[test]
    fn a_meter_kind_builds_only_its_own_histogram() {
        // Fuel is the one that costs the guest, so only `fuel` builds it.
        assert!(!Meters::new(MeterKind::Off).fuel_consumption.is_enabled());
        assert!(
            !Meters::new(MeterKind::Duration)
                .fuel_consumption
                .is_enabled()
        );
        assert!(Meters::new(MeterKind::Fuel).fuel_consumption.is_enabled());
        // Duration is what an operator asks for first, so anything but `off`
        // records it — including `fuel`.
        assert!(!Meters::new(MeterKind::Off).invocation.is_enabled());
        assert!(Meters::new(MeterKind::Duration).invocation.is_enabled());
        assert!(Meters::new(MeterKind::Fuel).invocation.is_enabled());
    }

    #[test]
    fn meter_kinds_parse_from_their_flag_spelling() {
        assert_eq!("off".parse::<MeterKind>(), Ok(MeterKind::Off));
        assert_eq!("duration".parse::<MeterKind>(), Ok(MeterKind::Duration));
        assert_eq!("fuel".parse::<MeterKind>(), Ok(MeterKind::Fuel));
        assert!("durations".parse::<MeterKind>().is_err());
        // The default is what a serverless host should run: rate, errors and
        // duration, which cost the guest two clock reads.
        assert_eq!(MeterKind::default(), MeterKind::Duration);
    }
}
