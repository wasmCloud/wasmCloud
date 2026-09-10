//! Where this process's telemetry goes: what the environment enables, the
//! resource it is attributed to, and the OTel providers that export it.
//!
//! [`install_providers`] builds the providers and installs no subscriber, so an
//! embedder with a `tracing` subscriber of its own can still export through
//! this configuration. [`super::initialize_observability`] is that plus the
//! `wash` CLI's own stderr layer.
//!
//! # Configuration
//!
//! Everything here is read from the environment, as part of the OpenTelemetry
//! specification. This is read in `runtime.env` in the Helm chart, `dev.environment`
//! in `.wash/config.yaml`, or the shell. There are deliberately no `--otel-*`
//! flags.
//!
//! The variables this runtime honors, all read once at startup:
//!
//! | variable | effect |
//! | --- | --- |
//! | `OTEL_SDK_DISABLED` | `true` exports nothing at all |
//! | `OTEL_EXPORTER_OTLP_ENDPOINT` | where every signal goes |
//! | `OTEL_EXPORTER_OTLP_{TRACES,LOGS,METRICS}_ENDPOINT` | where one signal goes, overriding the above |
//! | `OTEL_{TRACES,LOGS,METRICS}_EXPORTER` | `none` turns one signal off; `otlp` is the default |
//! | `OTEL_EXPORTER_OTLP_PROTOCOL` | `grpc` (default) or `http/protobuf` |
//! | `OTEL_EXPORTER_OTLP_{TRACES,LOGS,METRICS}_PROTOCOL` | the same, for one signal |
//! | `OTEL_EXPORTER_OTLP_TIMEOUT` | how long one export may take, in milliseconds |
//! | `OTEL_EXPORTER_OTLP_{TRACES,LOGS,METRICS}_TIMEOUT` | the same, for one signal |
//! | `OTEL_EXPORTER_OTLP_CERTIFICATE` | PEM bundle to trust when verifying the collector |
//! | `OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE` | PEM chain to present to the collector |
//! | `OTEL_EXPORTER_OTLP_CLIENT_KEY` | its key; both are required together |
//! | `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES` | read by the SDK's own detectors |
//!
//! A signal exports when an endpoint is configured for it and nothing turned it
//! off. Anything named but unusable is reported and skipped rather than fatal:
//! a mistyped telemetry variable must not stop the host.
//!
//! Not honored: per-signal `..._CERTIFICATE` and `..._CLIENT_*` (they would
//! need a connection pool per signal), `OTEL_EXPORTER_OTLP_HEADERS` and
//! `..._COMPRESSION` (read by `opentelemetry-otlp` itself, not by us), and
//! `http/json`, which this build cannot encode.
//!
//! `OTEL_EXPORTER_OTLP_INSECURE` is not honored either, and cannot be: the
//! specification defines it for a gRPC endpoint given without a scheme, and a
//! schemeless endpoint is rejected here. The endpoint's scheme is what decides
//! transport security. Because an operator can reasonably read that variable as
//! a TLS switch — and `opentelemetry-otlp` ignores it too — setting it to
//! `false` against an `http://` collector is called out at startup rather than
//! silently exporting in cleartext.

use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::Context as _;
use opentelemetry::{Key, KeyValue, trace::TracerProvider as _};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{WithHttpConfig as _, WithTonicConfig as _};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_semantic_conventions::resource;
use rustls::pki_types::pem::PemObject as _;
use tracing_subscriber::Layer;
use tracing_subscriber::registry::LookupSpan;

/// Flushes the OTel exporters, if [`install_providers`] installed any.
///
/// **Blocks** for up to five seconds per provider. This is the SDK's own timeout
/// so an async exit path calls [`flush_within`] instead of this. Runs at most
/// once; a no-op when no exporter was installed.
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

/// What a process leaving on a signal gives the exporters before it goes
/// without them. Only ever spent when one was configured.
///
/// Not the SDK's own five seconds per provider: three providers would be the
/// whole 15s grace period a terminating host pod gets, and
/// [`crate::host::Host::stop`] still has to unbind its workloads inside it.
pub const FLUSH_BUDGET: Duration = Duration::from_secs(2);

/// [`flush`], bounded: hands the exporters at most `budget` to deliver what
/// they were still batching, and answers whether they finished.
///
/// A blocking thread, because `flush` blocks: the OTLP exporter drains over the
/// connection this runtime has to keep polling. `false` covers both a budget
/// that ran out and an exporter that panicked on its way out — either way the
/// caller is leaving without the telemetry.
pub async fn flush_within(budget: Duration) -> bool {
    let flushed = tokio::task::spawn_blocking(flush);
    matches!(tokio::time::timeout(budget, flushed).await, Ok(Ok(())))
}

/// Set once by [`install_providers`], so [`flush`] can reach the providers it
/// built without every exit path having to be handed them.
static SHUTDOWN: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

/// `OTEL_SDK_DISABLED` from the OpenTelemetry environment specification. The
/// endpoint variables are spelled by `opentelemetry_otlp`, so this is the only
/// one named here.
const OTEL_SDK_DISABLED: &str = "OTEL_SDK_DISABLED";

/// The instrumentation scope this runtime's spans are recorded under.
const TRACER_NAME: &str = "runtime";

/// Whether `OTEL_SDK_DISABLED` switches the SDK off. Only the specification's
/// `true` does; anything else, an unparseable value included, leaves it on.
fn sdk_disabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
}

/// An endpoint variable and its value, when it names one. Unset, empty and
/// whitespace-only all read as unconfigured, which is what the SDK's own
/// endpoint resolution does with them.
fn configured<'a>(var: &'a str, value: Option<&'a str>) -> Option<(&'a str, &'a str)> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| (var, value))
}

fn validate_otlp_endpoint(var: &str, value: &str) -> Option<String> {
    let Some((scheme, _)) = value.split_once("://") else {
        return Some(format!(
            "missing scheme in {var} ({value}): expected http:// or https://"
        ));
    };

    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Some(format!(
            "unsupported OTLP endpoint scheme '{scheme}' in {var} ({value}): expected http:// or https://"
        ));
    }

    if let Err(e) = value.parse::<http::Uri>() {
        return Some(format!("invalid OTLP endpoint URL in {var} ({value}): {e}"));
    }

    None
}

/// Whether `OTEL_<SIGNAL>_EXPORTER` leaves an OTLP exporter selected.
///
/// Unset means the specification's default, `otlp`. `none` is how it turns one
/// signal off — a deliberate choice, so it is silent. Any other value names an
/// exporter this runtime does not implement, which is worth saying rather than
/// exporting over OTLP as though it had not been asked for.
fn otlp_selected(exporter: Option<(&str, &str)>) -> (bool, Option<String>) {
    let Some((var, value)) = exporter else {
        return (true, None);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "otlp" => (true, None),
        "none" => (false, None),
        other => (
            false,
            Some(format!(
                "unsupported exporter '{other}' in {var}: this runtime exports over OTLP only"
            )),
        ),
    }
}

/// Whether one signal's exporter is wanted, and what to tell the operator when
/// an endpoint or exporter was named but cannot be used.
///
/// The SDK is on, this signal still selects OTLP, and an endpoint is configured
/// for it — its own, or the shared base, in that order, which is the precedence
/// the SDK resolves them with. Keyed on endpoints rather than on the presence of
/// any `OTEL_`-prefixed variable, because an exporter built without one aims at
/// the OTLP default and retries against nothing for the life of the process.
///
/// Answered per signal for two reasons: the metrics exporter is the one with a
/// standing cost and traces have to be able to run without it, and an unusable
/// `..._LOGS_ENDPOINT` should silence logs alone rather than every signal.
fn signal_enabled(
    sdk_disabled: bool,
    exporter: Option<(&str, &str)>,
    base: Option<(&str, &str)>,
    signal: Option<(&str, &str)>,
) -> (bool, Option<String>) {
    if sdk_disabled {
        return (false, None);
    }
    let (selected, message) = otlp_selected(exporter);
    if !selected {
        return (false, message);
    }
    let Some((var, value)) = signal.or(base) else {
        return (false, None);
    };
    match validate_otlp_endpoint(var, value) {
        Some(message) => (false, Some(message)),
        None => (true, None),
    }
}

/// [`sdk_disabled`] over this process's environment, read once.
///
/// Cached like [`crate::timeouts`]: `wash`'s `main` exports a `wash dev`
/// project's `dev.environment` *before* observability initializes precisely
/// because these reads are one-shot, and keeping each read inside its accessor
/// keeps that contract in one place instead of at every call site.
pub(crate) fn otel_sdk_disabled() -> bool {
    static VALUE: LazyLock<bool> =
        LazyLock::new(|| sdk_disabled(std::env::var(OTEL_SDK_DISABLED).ok().as_deref()));
    *VALUE
}

/// The specification's per-signal exporter selectors, which the SDK crates do
/// not spell for us.
const OTEL_TRACES_EXPORTER: &str = "OTEL_TRACES_EXPORTER";
const OTEL_LOGS_EXPORTER: &str = "OTEL_LOGS_EXPORTER";
const OTEL_METRICS_EXPORTER: &str = "OTEL_METRICS_EXPORTER";

/// [`signal_enabled`] over this process's environment, for the signal selected
/// by `signal_exporter_var` and aimed by `signal_endpoint_var`.
fn enabled_from_env(
    signal_exporter_var: &str,
    signal_endpoint_var: &str,
) -> (bool, Option<String>) {
    let exporter = std::env::var(signal_exporter_var).ok();
    let base = std::env::var(opentelemetry_otlp::OTEL_EXPORTER_OTLP_ENDPOINT).ok();
    let signal = std::env::var(signal_endpoint_var).ok();
    signal_enabled(
        otel_sdk_disabled(),
        configured(signal_exporter_var, exporter.as_deref()),
        configured(
            opentelemetry_otlp::OTEL_EXPORTER_OTLP_ENDPOINT,
            base.as_deref(),
        ),
        configured(signal_endpoint_var, signal.as_deref()),
    )
}

fn traces() -> &'static (bool, Option<String>) {
    static VALUE: LazyLock<(bool, Option<String>)> = LazyLock::new(|| {
        enabled_from_env(
            OTEL_TRACES_EXPORTER,
            opentelemetry_otlp::OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
        )
    });
    &VALUE
}

fn logs() -> &'static (bool, Option<String>) {
    static VALUE: LazyLock<(bool, Option<String>)> = LazyLock::new(|| {
        enabled_from_env(
            OTEL_LOGS_EXPORTER,
            opentelemetry_otlp::OTEL_EXPORTER_OTLP_LOGS_ENDPOINT,
        )
    });
    &VALUE
}

fn metrics() -> &'static (bool, Option<String>) {
    static VALUE: LazyLock<(bool, Option<String>)> = LazyLock::new(|| {
        enabled_from_env(
            OTEL_METRICS_EXPORTER,
            opentelemetry_otlp::OTEL_EXPORTER_OTLP_METRICS_ENDPOINT,
        )
    });
    &VALUE
}

/// Whether to export traces. Read once; see [`otel_sdk_disabled`].
pub(crate) fn otel_traces_enabled() -> bool {
    traces().0
}

/// Whether to export logs. Read once; see [`otel_sdk_disabled`].
pub(crate) fn otel_logs_enabled() -> bool {
    logs().0
}

/// Whether to export metrics. Read once; see [`otel_sdk_disabled`].
pub(crate) fn otel_metrics_enabled() -> bool {
    metrics().0
}

/// Whether any exporter at all is wanted.
pub(crate) fn otel_enabled() -> bool {
    otel_traces_enabled() || otel_logs_enabled() || otel_metrics_enabled()
}

/// Report every endpoint that was named but cannot be used.
///
/// Called after the subscriber is installed, because that is the only way the
/// operator sees it — enablement is resolved before there is one. Deduplicated,
/// so an unusable shared endpoint is said once rather than once per signal.
pub fn log_configuration() {
    let mut warnings: Vec<&str> = [traces(), logs(), metrics()]
        .into_iter()
        .filter_map(|(_, message)| message.as_deref())
        .collect();
    warnings.sort_unstable();
    warnings.dedup();
    for warning in warnings {
        tracing::warn!("{warning}; continuing without OTLP export");
    }

    // Where telemetry is going, in the operator's own terms: the endpoint they
    // set and the protocol they chose. Without this a host that cannot reach
    // its collector looks identical to one exporting happily.
    //
    // One line per signal, because each resolves its own endpoint and protocol:
    // a single line would have to pick one signal's answer and present it as
    // everyone's, which is wrong exactly when an operator has split them.
    let client_auth = otlp_client_identity().is_some();
    for (signal, config, protocol_var, endpoint_var, insecure_var) in [
        (
            "traces",
            traces(),
            OTEL_EXPORTER_OTLP_TRACES_PROTOCOL,
            opentelemetry_otlp::OTEL_EXPORTER_OTLP_TRACES_ENDPOINT,
            "OTEL_EXPORTER_OTLP_TRACES_INSECURE",
        ),
        (
            "logs",
            logs(),
            OTEL_EXPORTER_OTLP_LOGS_PROTOCOL,
            opentelemetry_otlp::OTEL_EXPORTER_OTLP_LOGS_ENDPOINT,
            "OTEL_EXPORTER_OTLP_LOGS_INSECURE",
        ),
        (
            "metrics",
            metrics(),
            OTEL_EXPORTER_OTLP_METRICS_PROTOCOL,
            opentelemetry_otlp::OTEL_EXPORTER_OTLP_METRICS_ENDPOINT,
            "OTEL_EXPORTER_OTLP_METRICS_INSECURE",
        ),
    ] {
        let (enabled, _) = config;
        if !enabled {
            continue;
        }
        let endpoint = signal_or_shared(
            endpoint_var,
            opentelemetry_otlp::OTEL_EXPORTER_OTLP_ENDPOINT,
        )
        .unwrap_or_default();
        let insecure = signal_or_shared(insecure_var, OTEL_EXPORTER_OTLP_INSECURE);
        if insecure_disagreement(insecure.as_deref(), &endpoint) {
            tracing::warn!(
                "{signal} are exported unencrypted: transport security is decided by the \
                 endpoint's scheme, and {endpoint} is plaintext. Use https:// — setting an OTLP \
                 `insecure` variable does not encrypt anything here"
            );
        }
        tracing::info!(
            signal,
            endpoint = endpoint,
            protocol = protocol_from_env(protocol_var).as_str(),
            client_auth,
            "exporting telemetry over OTLP"
        );
    }
}

/// `OTEL_EXPORTER_OTLP_INSECURE` and its per-signal forms.
///
/// The specification defines these for a gRPC endpoint given *without* a
/// scheme. This runtime rejects a schemeless endpoint outright, so the scheme is
/// always what decides transport security and these can never change it — which
/// is exactly why they are worth checking: `opentelemetry-otlp` does not read
/// them either, so an operator who sets `false` and believes they have
/// encryption gets plaintext with nothing said.
const OTEL_EXPORTER_OTLP_INSECURE: &str = "OTEL_EXPORTER_OTLP_INSECURE";

/// Whether the operator asked for transport security the endpoint does not give
/// them: an `insecure` variable set to `false` against an `http://` collector.
///
/// Only that direction. `true` against `https://` is also a mistaken belief,
/// but it errs toward encryption and would fire on the common, harmless case of
/// a plaintext in-cluster collector with the variable left at its default.
fn insecure_disagreement(insecure: Option<&str>, endpoint: &str) -> bool {
    let asked_for_security =
        insecure.is_some_and(|value| value.trim().eq_ignore_ascii_case("false"));
    asked_for_security && endpoint.trim().to_ascii_lowercase().starts_with("http://")
}

/// The value governing one signal: its own variable, else the shared one.
///
/// The same precedence [`signal_enabled`] resolves endpoints with, so the line
/// an operator reads names the value actually in force rather than only the
/// shared variable — which is absent exactly when they aimed one signal
/// somewhere else.
fn signal_or_shared(signal_var: &str, shared_var: &str) -> Option<String> {
    std::env::var(signal_var)
        .ok()
        .or_else(|| std::env::var(shared_var).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// What `SdkProvidedResourceDetector` emits for `service.name` when nothing
/// named the service. The Rust SDK uses the bare literal, not the
/// specification's `unknown_service:<executable>`.
const UNKNOWN_SERVICE: &str = "unknown_service";

/// This runtime's own `service.name`, or `None` when the environment already
/// named one.
///
/// Presence cannot be the test: `SdkProvidedResourceDetector` *always* emits
/// `service.name`, falling back to [`UNKNOWN_SERVICE`] when neither
/// `OTEL_SERVICE_NAME` nor `OTEL_RESOURCE_ATTRIBUTES` supplied a value. That
/// sentinel is the only thing separating "nobody named it" from a real name.
fn default_service_name(detected: &Resource) -> Option<KeyValue> {
    let named = detected
        .get(&Key::from_static_str(resource::SERVICE_NAME))
        .is_some_and(|value| value.as_str() != UNKNOWN_SERVICE);
    (!named).then(|| KeyValue::new(resource::SERVICE_NAME.to_string(), env!("CARGO_PKG_NAME")))
}

/// The resource every provider this process builds carries.
///
/// `Resource::builder` runs the SDK, telemetry and environment detectors, so
/// `OTEL_SERVICE_NAME` and `OTEL_RESOURCE_ATTRIBUTES` are read before we add
/// anything. `service.version` and `service.instance.id` are this runtime's to
/// state; `service.name` is the operator's whenever they set one, because a
/// fleet that all reports as `wash-runtime` cannot be told apart.
fn resource() -> Resource {
    let detected = Resource::builder().build();
    let mut builder = Resource::builder()
        .with_attribute(KeyValue::new(
            resource::SERVICE_INSTANCE_ID.to_string(),
            uuid::Uuid::new_v4().to_string(),
        ))
        .with_attribute(KeyValue::new(
            resource::SERVICE_VERSION.to_string(),
            env!("CARGO_PKG_VERSION"),
        ));
    if let Some(service_name) = default_service_name(&detected) {
        builder = builder.with_attribute(service_name);
    }
    builder.build()
}

/// The OTel providers this process installed, and the `tracing` layers that
/// feed them.
///
/// Held by whoever called [`install_providers`] only for the layers: the
/// shutdown [`flush`] runs already owns its own handles, so dropping this does
/// not stop anything exporting.
pub struct Providers {
    logs: Option<SdkLoggerProvider>,
    traces: Option<SdkTracerProvider>,
    metrics: Option<SdkMeterProvider>,
}

impl Providers {
    /// The layer that exports `tracing` events as OTel logs. `None` when logs
    /// are not enabled.
    ///
    /// Unfiltered: an embedder's `Registry` has its own idea of what to record,
    /// so the caller composes its own `.with_filter(..)`.
    pub fn logs_layer<S>(&self) -> Option<impl Layer<S> + use<S>>
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    {
        self.logs.as_ref().map(OpenTelemetryTracingBridge::new)
    }

    /// The layer that exports `tracing` spans as OTel traces. `None` when
    /// traces are not enabled. Unfiltered, as [`Self::logs_layer`] is.
    pub fn tracing_layer<S>(&self) -> Option<impl Layer<S> + use<S>>
    where
        S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    {
        self.traces.as_ref().map(|provider| {
            tracing_opentelemetry::layer()
                .with_tracer(provider.tracer(TRACER_NAME))
                .with_error_records_to_exceptions(true)
                .with_error_fields_to_exceptions(true)
                .with_error_events_to_status(true)
                .with_error_events_to_exceptions(true)
                .with_location(true)
        })
    }

    /// The meter provider, when metrics are enabled. Already registered as the
    /// global one; returned so an embedder can build [`super::Meters`] against
    /// it explicitly rather than through the global.
    pub fn meter_provider(&self) -> Option<&SdkMeterProvider> {
        self.metrics.as_ref()
    }

    /// Whether any exporter at all was installed. Metrics count: they are
    /// exported without a layer, so the two layer accessors do not see them.
    pub fn any(&self) -> bool {
        self.logs.is_some() || self.traces.is_some() || self.metrics.is_some()
    }
}

/// Builds the resource and whichever providers the environment enables,
/// registers the global meter provider and the W3C propagator, and records the
/// shutdown hook [`flush`] runs. **Installs no subscriber**: the caller adds
/// [`Providers::logs_layer`] and [`Providers::tracing_layer`] to a `Registry`
/// of its own.
///
/// Registering the global meter provider is what puts this runtime's
/// instruments — `guest.invocation.duration` and `fuel.consumption`, both bound
/// at construction to `opentelemetry::global::meter` — on these exporters. An
/// embedder that wants its own provider behind them sets that instead of
/// calling this.
///
/// At most once per process. A second call builds providers whose shutdown hook
/// cannot be recorded, because [`flush`] reaches exactly one, so it returns an
/// error rather than leaving exporters nothing will ever drain.
pub fn install_providers() -> anyhow::Result<Providers> {
    // Claimed before anything is built, and atomically: registering the
    // shutdown hook is the step that can discover a second call, and by then
    // the exporters would already be running with no way for `flush` to ever
    // reach them. A plain `SHUTDOWN.get().is_none()` would let two concurrent
    // callers both past the check into exactly that state.
    static CLAIMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    anyhow::ensure!(
        !CLAIMED.swap(true, std::sync::atomic::Ordering::SeqCst),
        "OTel providers were already installed in this process"
    );

    let resource = resource();

    let logs = if otel_logs_enabled() {
        let exporter = build_log_exporter().context("failed to build the OTLP log exporter")?;
        Some(
            opentelemetry_sdk::logs::LoggerProviderBuilder::default()
                .with_batch_exporter(exporter)
                .with_resource(resource.clone())
                .build(),
        )
    } else {
        None
    };

    let traces = if otel_traces_enabled() {
        let exporter = build_span_exporter().context("failed to build the OTLP span exporter")?;
        Some(
            opentelemetry_sdk::trace::TracerProviderBuilder::default()
                .with_batch_exporter(exporter)
                .with_resource(resource.clone())
                .build(),
        )
    } else {
        None
    };

    let metrics = if otel_metrics_enabled() {
        let exporter =
            build_metric_exporter().context("failed to build the OTLP metric exporter")?;
        let provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_periodic_exporter(exporter)
            .with_resource(resource)
            .build();
        opentelemetry::global::set_meter_provider(provider.clone());
        Some(provider)
    } else {
        None
    };

    // Register the W3C Trace Context propagator so the incoming-request path
    // (`opentelemetry::global::get_text_map_propagator` in `host::http`) can
    // parse the `traceparent` header into the OpenTelemetry context.
    // Without this every workload roots its own trace instead of continuing the
    // caller's. Registering it here is what lets a trace roll up across
    // workload/host boundaries.
    //
    // Registered for any enabled signal, not just traces: `SdkLogger::emit`
    // stamps the current context's span onto every record, so a logs-only host
    // is exactly the one that needs the caller's `traceparent` parsed.
    if logs.is_some() || traces.is_some() || metrics.is_some() {
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );
    }

    register_shutdown(logs.clone(), traces.clone(), metrics.clone())?;

    Ok(Providers {
        logs,
        traces,
        metrics,
    })
}

/// Records what [`flush`] shuts down. Registered rather than only returned:
/// every way this process can end has to be able to flush, and `main` is not on
/// all of them.
fn register_shutdown(
    logs: Option<SdkLoggerProvider>,
    traces: Option<SdkTracerProvider>,
    metrics: Option<SdkMeterProvider>,
) -> anyhow::Result<()> {
    if logs.is_none() && traces.is_none() && metrics.is_none() {
        // Nothing to flush; leave the slot free for a later caller that does
        // install exporters.
        return Ok(());
    }
    SHUTDOWN
        .set(Box::new(move || {
            if let Some(traces) = &traces
                && let Err(e) = traces.shutdown()
            {
                eprintln!("failed to shutdown tracer provider: {e}");
            }
            if let Some(logs) = &logs
                && let Err(e) = logs.shutdown()
            {
                eprintln!("failed to shutdown log provider: {e}");
            }
            if let Some(metrics) = &metrics
                && let Err(e) = metrics.shutdown()
            {
                eprintln!("failed to shutdown meter provider: {e}");
            }
        }))
        .map_err(|_| anyhow::anyhow!("OTel providers were already installed in this process"))
}

/// Which OTLP wire protocol an exporter speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Protocol {
    /// OTLP/gRPC. The OpenTelemetry default, and this runtime's.
    Grpc,
    /// OTLP/HTTP carrying binary protobuf — what reaches a collector behind an
    /// ingress that will not carry gRPC.
    HttpBinary,
}

impl Protocol {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Grpc => "grpc",
            Self::HttpBinary => "http/protobuf",
        }
    }
}

/// `OTEL_EXPORTER_OTLP_TRACES_PROTOCOL` and its siblings, which
/// `opentelemetry_otlp` does not spell for us.
const OTEL_EXPORTER_OTLP_TRACES_PROTOCOL: &str = "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL";
const OTEL_EXPORTER_OTLP_LOGS_PROTOCOL: &str = "OTEL_EXPORTER_OTLP_LOGS_PROTOCOL";
const OTEL_EXPORTER_OTLP_METRICS_PROTOCOL: &str = "OTEL_EXPORTER_OTLP_METRICS_PROTOCOL";

/// The protocol one signal exports over: its own variable, then the shared one,
/// then gRPC.
///
/// `http/json` is an error rather than a fallback. It is a protocol the
/// specification defines and `opentelemetry-otlp` implements behind an
/// `http-json` feature we do not build with — and without that feature its
/// encoder arm is compiled out, so asking for JSON silently gets protobuf. An
/// operator who set this deliberately deserves to hear that it did not happen.
fn resolve_protocol(signal: Option<&str>, base: Option<&str>) -> anyhow::Result<Protocol> {
    let Some(value) = signal
        .or(base)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(Protocol::Grpc);
    };
    match value {
        "grpc" => Ok(Protocol::Grpc),
        "http/protobuf" => Ok(Protocol::HttpBinary),
        "http/json" => anyhow::bail!(
            "OTLP protocol `http/json` is not supported by this build; \
             use `http/protobuf` or `grpc`"
        ),
        other => {
            anyhow::bail!("unknown OTLP protocol `{other}`, expected one of: grpc, http/protobuf")
        }
    }
}

/// [`resolve_protocol`] over this process's environment, falling back to gRPC
/// on anything it rejects.
///
/// A misconfigured exporter must not take the process with it: this runs on
/// every `wash` invocation, so a typo here would otherwise stop `wash build`.
/// Written to stderr rather than through `tracing`, because the subscriber is
/// not installed until the providers this feeds are built.
fn protocol_from_env(signal_protocol_var: &str) -> Protocol {
    static REPORTED: LazyLock<std::sync::Mutex<std::collections::BTreeSet<String>>> =
        LazyLock::new(Default::default);
    match resolve_protocol(
        std::env::var(signal_protocol_var).ok().as_deref(),
        std::env::var(opentelemetry_otlp::OTEL_EXPORTER_OTLP_PROTOCOL)
            .ok()
            .as_deref(),
    ) {
        Ok(protocol) => protocol,
        Err(e) => {
            // Once per variable: this resolves again for every exporter built,
            // and repeating the same complaint three times helps nobody.
            if let Ok(mut reported) = REPORTED.lock()
                && reported.insert(signal_protocol_var.to_string())
            {
                report(&format!("{e:#}; exporting over gRPC instead"));
            }
            Protocol::Grpc
        }
    }
}

/// Tell the operator about a setting we could not use, from a path that runs
/// before the subscriber exists.
///
/// Straight to stderr, because `initialize_observability` resolves all of this
/// while building the providers the subscriber is composed from — a
/// `tracing::warn!` here would have nothing to carry it. The endpoint warnings
/// go through `warn_about_unusable_endpoints` instead, because enablement is
/// resolved once and can be replayed after the subscriber is installed.
fn report(message: &str) {
    eprintln!("{message}");
}

/// How long one export may take, resolved as the SDK resolves it: the signal's
/// own variable, then the shared one, then ten seconds. Whole milliseconds.
///
/// Needed here because a supplied client carries its own timeout — the SDK only
/// applies this to the client it would have built itself.
/// A set-but-unparseable value is reported and then ignored, as
/// [`crate::timeouts`] does with its own overrides — silently falling back
/// would leave an operator's typo undetected, and this is the same class of
/// mistake `protocol_from_env` reports.
fn resolve_timeout(signal_timeout_var: &str) -> Duration {
    let millis = |var: &str| -> Option<u64> {
        let value = std::env::var(var).ok()?;
        match value.trim().parse() {
            Ok(millis) => Some(millis),
            Err(_) => {
                report(&format!(
                    "ignoring unparseable export timeout '{value}' in {var} (want whole \
                     milliseconds)"
                ));
                None
            }
        }
    };
    millis(signal_timeout_var)
        .or_else(|| millis(opentelemetry_otlp::OTEL_EXPORTER_OTLP_TIMEOUT))
        .map_or(
            opentelemetry_otlp::OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT,
            Duration::from_millis,
        )
}

/// `OTEL_EXPORTER_OTLP_CERTIFICATE` from the OpenTelemetry environment
/// specification: a PEM bundle to trust when verifying the collector.
const OTEL_EXPORTER_OTLP_CERTIFICATE: &str = "OTEL_EXPORTER_OTLP_CERTIFICATE";

/// The specification's client-authentication pair, for a collector that asks
/// for a certificate of its own.
const OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE: &str = "OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE";
const OTEL_EXPORTER_OTLP_CLIENT_KEY: &str = "OTEL_EXPORTER_OTLP_CLIENT_KEY";

/// The client certificate and key to present to a collector, when both are
/// named.
///
/// Both or neither: a certificate without its key, or the reverse, cannot
/// authenticate anything, and silently connecting anonymously would look like
/// working mTLS until the collector refused it.
fn otlp_client_identity() -> Option<&'static (std::path::PathBuf, std::path::PathBuf)> {
    static VALUE: LazyLock<Option<(std::path::PathBuf, std::path::PathBuf)>> =
        LazyLock::new(resolve_client_identity);
    VALUE.as_ref()
}

/// [`otlp_client_identity`] before caching, so its diagnostic is emitted once
/// rather than once per exporter built.
fn resolve_client_identity() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let certificate = env_path(OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE);
    let key = env_path(OTEL_EXPORTER_OTLP_CLIENT_KEY);
    match (certificate, key) {
        (Some(certificate), Some(key)) => Some((certificate, key)),
        (None, None) => None,
        (certificate, _) => {
            let missing = if certificate.is_some() {
                OTEL_EXPORTER_OTLP_CLIENT_KEY
            } else {
                OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE
            };
            report(&format!(
                "ignoring OTLP client authentication: {missing} is not set, and a certificate \
                 without its key cannot authenticate"
            ));
            None
        }
    }
}

/// A path-valued variable, when it names one.
fn env_path(var: &str) -> Option<std::path::PathBuf> {
    std::env::var(var)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

/// The trust roots the exporters verify a collector against.
///
/// A collector behind a private CA is the ordinary case for OTLP over TLS, and
/// the specification gives it its own variable rather than borrowing the trust
/// a workload's egress uses — the two are different boundaries, and an operator
/// should be able to widen one without the other. Layered on this host's
/// default roots, so naming a private CA does not stop the public ones working.
///
/// A bundle we cannot load is reported and skipped rather than fatal: the
/// collector becomes unreachable, which is what the operator will see, but a
/// telemetry misconfiguration does not stop the host.
fn otlp_trust_roots() -> Arc<rustls::ClientConfig> {
    static CONFIG: LazyLock<Arc<rustls::ClientConfig>> =
        LazyLock::new(|| {
            match otlp_client_config(otlp_certificate_path().as_deref(), otlp_client_identity()) {
                Ok(config) => config,
                Err(e) => {
                    report(&format!("ignoring OTLP TLS configuration: {e:#}"));
                    crate::host::http_client::default_client_tls_config()
                }
            }
        });
    CONFIG.clone()
}

/// The OTLP/HTTP rustls configuration: this host's default roots plus any CA
/// the environment named, and a client certificate when it asked for one.
///
/// Takes its inputs rather than reading them, because the accessors that supply
/// them cache for the process and could not otherwise be exercised.
fn otlp_client_config(
    certificate: Option<&std::path::Path>,
    identity: Option<&(std::path::PathBuf, std::path::PathBuf)>,
) -> anyhow::Result<Arc<rustls::ClientConfig>> {
    if certificate.is_none() && identity.is_none() {
        return Ok(crate::host::http_client::default_client_tls_config());
    }

    let roots = crate::host::http_client::ClientTlsOptions {
        extra_ca_paths: certificate.map(Path::to_path_buf).into_iter().collect(),
        ..Default::default()
    }
    .root_store()?;
    let builder = rustls::ClientConfig::builder().with_root_certificates(roots);

    let Some((certificate, key)) = identity else {
        return Ok(Arc::new(builder.with_no_client_auth()));
    };
    let chain = rustls::pki_types::CertificateDer::pem_file_iter(certificate)
        .with_context(|| format!("failed to read {}", certificate.display()))?
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("failed to parse PEM in {}", certificate.display()))?;
    let key = rustls::pki_types::PrivateKeyDer::from_pem_file(key)
        .with_context(|| format!("failed to read client key {}", key.display()))?;
    Ok(Arc::new(
        builder
            .with_client_auth_cert(chain, key)
            .context("failed to build the OTLP client certificate")?,
    ))
}

/// The gRPC exporters' TLS configuration.
///
/// Attached unconditionally: tonic applies it only when the endpoint's scheme
/// is `https`, so an `http://` collector is unaffected, and without it tonic
/// refuses an `https://` one outright with "Connecting to HTTPS without TLS
/// enabled".
///
/// Tonic builds its own root store rather than taking a `rustls::ClientConfig`,
/// so this reads `OTEL_EXPORTER_OTLP_CERTIFICATE` directly instead of going
/// through [`otlp_trust_roots`] — same variable, layered the same way.
///
/// The built-in roots are named rather than taken from `with_enabled_roots`,
/// which resolves to whichever root features the `tonic` dependency happens to
/// carry. That would make the trust anchors differ by protocol — native roots
/// over gRPC, webpki over HTTP — so the same collector could verify one way and
/// not the other, and trimming an unrelated `tonic` feature would empty this
/// store silently. These are the roots [`super::super::host::http_client`]
/// trusts by default, so both transports agree.
fn grpc_tls_config() -> opentelemetry_otlp::tonic_types::transport::ClientTlsConfig {
    use opentelemetry_otlp::tonic_types::transport::{Certificate, ClientTlsConfig, Identity};

    let mut config =
        ClientTlsConfig::new().trust_anchors(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(path) = otlp_certificate_path() {
        match std::fs::read(&path) {
            Ok(pem) => config = config.ca_certificate(Certificate::from_pem(pem)),
            Err(e) => report(&format!(
                "ignoring {OTEL_EXPORTER_OTLP_CERTIFICATE} ({}): {e}",
                path.display()
            )),
        }
    }
    if let Some((certificate, key)) = otlp_client_identity() {
        match (std::fs::read(certificate), std::fs::read(key)) {
            (Ok(certificate), Ok(key)) => {
                config = config.identity(Identity::from_pem(certificate, key))
            }
            (Err(e), _) => report(&format!(
                "ignoring OTLP client authentication: failed to read {} ({e})",
                certificate.display()
            )),
            (_, Err(e)) => report(&format!(
                "ignoring OTLP client authentication: failed to read {} ({e})",
                key.display()
            )),
        }
    }
    config
}

/// `OTEL_EXPORTER_OTLP_CERTIFICATE`, when it names a bundle.
fn otlp_certificate_path() -> Option<std::path::PathBuf> {
    std::env::var(OTEL_EXPORTER_OTLP_CERTIFICATE)
        .ok()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .map(std::path::PathBuf::from)
}

/// The connection pool every OTLP/HTTP exporter shares.
///
/// One pool rather than one per signal: the three exporters talk to the same
/// collector, and a pool apiece would mean three TLS handshakes and three sets
/// of idle connections to it. Built on the connector the rest of this host's
/// egress uses, with a timer so idle connections actually expire rather than
/// being reused after a collector or load balancer has dropped them.
fn otlp_pool() -> &'static hyper_util::client::legacy::Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    http_body_util::Full<bytes::Bytes>,
> {
    static POOL: LazyLock<
        hyper_util::client::legacy::Client<
            hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
            http_body_util::Full<bytes::Bytes>,
        >,
    > = LazyLock::new(|| {
        let connector = crate::host::http_client::https_connector(
            &otlp_trust_roots(),
            crate::host::http_client::Alpn::Http1,
        );
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new())
            .pool_timer(hyper_util::rt::TokioTimer::new())
            .pool_idle_timeout(Duration::from_secs(90))
            .build(connector)
    });
    &POOL
}

/// The client the OTLP/HTTP exporters send over, bound to the runtime that
/// built it and carrying this signal's export timeout.
///
/// The SDK exports from a thread of its own — `thread::Builder::new()` driving
/// `futures_executor::block_on` — where a hyper client has neither a reactor to
/// time out against nor an executor to drive its connections, and panics with
/// "there is no reactor running" on its first send. Sending through a captured
/// handle is what gives it both; the join handle it awaits is woken by that
/// runtime, so the SDK's own thread can block on it.
///
/// The timeout lives here rather than on the pool because it is per signal, and
/// because `opentelemetry-otlp` ignores the one it resolves whenever a client is
/// supplied.
#[derive(Debug, Clone)]
struct OtlpHttpClient {
    handle: tokio::runtime::Handle,
    timeout: Duration,
}

#[async_trait::async_trait]
impl opentelemetry_http::HttpClient for OtlpHttpClient {
    async fn send_bytes(
        &self,
        request: http::Request<bytes::Bytes>,
    ) -> Result<http::Response<bytes::Bytes>, opentelemetry_http::HttpError> {
        use http_body_util::BodyExt as _;

        let timeout = self.timeout;
        self.handle
            .spawn(async move {
                let (parts, body) = request.into_parts();
                let request = http::Request::from_parts(parts, http_body_util::Full::new(body));
                // The whole exchange, not just the head: a collector that
                // answers and then stalls mid-body would otherwise wedge the
                // SDK's export thread for good, and this is the only timeout in
                // the path — `opentelemetry-otlp` ignores the one it resolves
                // whenever a client is supplied.
                tokio::time::timeout(timeout, async {
                    let response = otlp_pool().request(request).await?;
                    let (parts, body) = response.into_parts();
                    let body = body.collect().await?.to_bytes();
                    Ok::<_, opentelemetry_http::HttpError>(http::Response::from_parts(parts, body))
                })
                .await?
            })
            .await
            .map_err(|e| Box::new(e) as opentelemetry_http::HttpError)?
    }
}

/// [`OtlpHttpClient`] for one signal, or the reason it cannot be built.
fn http_client(timeout: Duration) -> anyhow::Result<OtlpHttpClient> {
    let handle = tokio::runtime::Handle::try_current().context(
        "OTLP/HTTP exporters must be built from within a Tokio runtime: the SDK exports from a \
         thread of its own and sends through a handle to this one",
    )?;
    Ok(OtlpHttpClient { handle, timeout })
}

pub(crate) fn build_log_exporter() -> anyhow::Result<opentelemetry_otlp::LogExporter> {
    let builder = opentelemetry_otlp::LogExporter::builder();
    Ok(match protocol_from_env(OTEL_EXPORTER_OTLP_LOGS_PROTOCOL) {
        Protocol::Grpc => builder
            .with_tonic()
            .with_tls_config(grpc_tls_config())
            .build()?,
        Protocol::HttpBinary => builder
            .with_http()
            .with_http_client(http_client(resolve_timeout(
                opentelemetry_otlp::OTEL_EXPORTER_OTLP_LOGS_TIMEOUT,
            ))?)
            .build()?,
    })
}

pub(crate) fn build_span_exporter() -> anyhow::Result<opentelemetry_otlp::SpanExporter> {
    let builder = opentelemetry_otlp::SpanExporter::builder();
    Ok(
        match protocol_from_env(OTEL_EXPORTER_OTLP_TRACES_PROTOCOL) {
            Protocol::Grpc => builder
                .with_tonic()
                .with_tls_config(grpc_tls_config())
                .build()?,
            Protocol::HttpBinary => builder
                .with_http()
                .with_http_client(http_client(resolve_timeout(
                    opentelemetry_otlp::OTEL_EXPORTER_OTLP_TRACES_TIMEOUT,
                ))?)
                .build()?,
        },
    )
}

pub(crate) fn build_metric_exporter() -> anyhow::Result<opentelemetry_otlp::MetricExporter> {
    let builder = opentelemetry_otlp::MetricExporter::builder();
    Ok(
        match protocol_from_env(OTEL_EXPORTER_OTLP_METRICS_PROTOCOL) {
            Protocol::Grpc => builder
                .with_tonic()
                .with_tls_config(grpc_tls_config())
                .build()?,
            Protocol::HttpBinary => builder
                .with_http()
                .with_http_client(http_client(resolve_timeout(
                    opentelemetry_otlp::OTEL_EXPORTER_OTLP_METRICS_TIMEOUT,
                ))?)
                .build()?,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One signal's endpoint, as the accessors assemble it.
    fn endpoint(value: &str) -> Option<(&str, &str)> {
        configured("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", Some(value))
    }

    /// The shared endpoint, likewise.
    fn base(value: &str) -> Option<(&str, &str)> {
        configured("OTEL_EXPORTER_OTLP_ENDPOINT", Some(value))
    }

    /// The enablement matrix, over the values rather than the environment: the
    /// accessors cache in a `LazyLock`, so whichever test ran first would fix
    /// the answer for the rest of the process.
    #[test]
    fn an_exporter_is_enabled_by_an_endpoint_not_by_any_otel_var() {
        let enabled = |disabled, base, signal| signal_enabled(disabled, None, base, signal).0;

        // The variable whose whole purpose is to turn telemetry off.
        assert!(!sdk_disabled(None));
        assert!(sdk_disabled(Some("true")));
        assert!(sdk_disabled(Some("TRUE")));
        assert!(!sdk_disabled(Some("false")));
        // Not a boolean at all: the specification names one value, and a typo
        // must not silently disable a fleet's telemetry.
        assert!(!sdk_disabled(Some("yes")));

        // An endpoint from either source enables; neither does not. Nothing
        // else — a bare `OTEL_SERVICE_NAME` names no destination.
        assert!(enabled(false, base("http://collector:4317"), None));
        assert!(enabled(false, None, endpoint("http://collector:4318")));
        assert!(!enabled(false, None, None));

        // Disabled beats any endpoint.
        assert!(!enabled(true, base("http://collector:4317"), None));

        // Set-but-empty is not configured: the SDK's own resolution discards
        // these too, so honoring them would aim an exporter at the default.
        assert!(!enabled(
            false,
            configured("OTEL_EXPORTER_OTLP_ENDPOINT", Some("")),
            configured("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", Some("   ")),
        ));
    }

    /// A signal's own endpoint wins over the shared one, and is the only one
    /// validated for that signal — so an unusable endpoint silences the signal
    /// that named it rather than every signal.
    #[test]
    fn a_signals_endpoint_overrides_the_shared_one() {
        let (enabled, warning) = signal_enabled(
            false,
            None,
            base("unix:///dev/otel.sock"),
            endpoint("http://collector:4318"),
        );
        assert!(enabled, "the signal's own endpoint is the one that applies");
        assert!(warning.is_none());

        // ... and the reverse: a bad endpoint on the signal is not rescued by a
        // good shared one.
        let (enabled, warning) = signal_enabled(
            false,
            None,
            base("http://collector:4317"),
            endpoint("unix:///dev/otel.sock"),
        );
        assert!(!enabled);
        let warning = warning.expect("an unusable endpoint is reported");
        assert!(warning.contains("unix"), "{warning}");
        assert!(
            warning.contains("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"),
            "the message names the variable the operator set: {warning}"
        );
    }

    #[test]
    fn unsupported_scheme_is_rejected_with_actionable_message() {
        let msg =
            validate_otlp_endpoint("OTEL_EXPORTER_OTLP_ENDPOINT", "unix:///dev/otel-grpc.sock")
                .expect("unix scheme should be rejected");
        assert!(
            msg.contains("unsupported OTLP endpoint scheme 'unix'"),
            "message was: {msg}"
        );
        assert!(
            msg.contains("expected http:// or https://"),
            "message was: {msg}"
        );
    }

    #[test]
    fn http_and_https_schemes_are_accepted() {
        assert_eq!(
            validate_otlp_endpoint("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4317"),
            None
        );
        assert_eq!(
            validate_otlp_endpoint("OTEL_EXPORTER_OTLP_ENDPOINT", "https://collector:4317"),
            None
        );
        assert_eq!(
            validate_otlp_endpoint("OTEL_EXPORTER_OTLP_ENDPOINT", "HTTP://localhost:4317"),
            None
        );
    }

    #[test]
    fn missing_scheme_is_rejected() {
        let msg = validate_otlp_endpoint("OTEL_EXPORTER_OTLP_ENDPOINT", "localhost:4317")
            .expect("value without a scheme should be rejected");
        assert!(msg.contains("missing scheme"), "message was: {msg}");
    }

    #[test]
    fn an_unusable_endpoint_disables_that_signal_and_says_why() {
        let (enabled, warning) =
            signal_enabled(false, None, base("unix:///dev/otel-grpc.sock"), None);
        assert!(!enabled);
        let warning = warning.expect("an unusable endpoint is reported");
        assert!(
            warning.contains("unsupported OTLP endpoint scheme 'unix'"),
            "{warning}"
        );

        // Disabling the SDK is not a misconfiguration, so it says nothing.
        assert_eq!(
            signal_enabled(true, None, base("unix:///dev/otel-grpc.sock"), None),
            (false, None)
        );
    }

    /// `service.name` is the operator's when they set one, and ours only when
    /// nobody did. Both cases turn on the sentinel: the detector always emits
    /// the key, so a presence test would never let our default apply.
    #[test]
    fn the_environment_names_the_service_and_we_only_default_it() {
        let named = |name: &str| {
            Resource::builder_empty()
                .with_attribute(KeyValue::new(
                    resource::SERVICE_NAME.to_string(),
                    name.to_string(),
                ))
                .build()
        };

        assert_eq!(
            default_service_name(&named(UNKNOWN_SERVICE)).map(|kv| kv.value.as_str().to_string()),
            Some(env!("CARGO_PKG_NAME").to_string()),
        );
        assert!(default_service_name(&named("checkout")).is_none());
        // Nothing detected at all is still nobody naming it.
        assert!(default_service_name(&Resource::builder_empty().build()).is_some());
    }

    /// The detector chain this rests on, end to end: `OTEL_SERVICE_NAME` has to
    /// survive into the resource every provider carries, and our version and
    /// instance id have to be there whether or not it did.
    #[test]
    fn otel_service_name_wins_over_our_default() {
        let service_name = |resource: &Resource| {
            resource
                .get(&Key::from_static_str(resource::SERVICE_NAME))
                .map(|value| value.as_str().to_string())
        };

        crate::env_guard::with_vars(
            [
                ("OTEL_SERVICE_NAME", Some("checkout")),
                ("OTEL_RESOURCE_ATTRIBUTES", None),
            ],
            || {
                let resource = resource();
                assert_eq!(service_name(&resource), Some("checkout".to_string()));
                assert_eq!(
                    resource
                        .get(&Key::from_static_str(resource::SERVICE_VERSION))
                        .map(|value| value.as_str().to_string()),
                    Some(env!("CARGO_PKG_VERSION").to_string()),
                );
                assert!(
                    resource
                        .get(&Key::from_static_str(resource::SERVICE_INSTANCE_ID))
                        .is_some()
                );
            },
        );

        crate::env_guard::with_vars(
            [
                ("OTEL_SERVICE_NAME", None::<&str>),
                ("OTEL_RESOURCE_ATTRIBUTES", None),
            ],
            || {
                assert_eq!(
                    service_name(&resource()),
                    Some(env!("CARGO_PKG_NAME").to_string())
                );
            },
        );

        // `OTEL_RESOURCE_ATTRIBUTES` names it too, and still beats our default.
        crate::env_guard::with_vars(
            [
                ("OTEL_SERVICE_NAME", None),
                ("OTEL_RESOURCE_ATTRIBUTES", Some("service.name=inventory")),
            ],
            || {
                assert_eq!(service_name(&resource()), Some("inventory".to_string()));
            },
        );
    }

    /// The protocol table, per signal and shared, with the two values we can
    /// actually speak and an error for everything else.
    #[test]
    fn a_signals_protocol_overrides_the_shared_one() {
        // gRPC is the default, and stays it for an unset or empty setting.
        assert_eq!(resolve_protocol(None, None).unwrap(), Protocol::Grpc);
        assert_eq!(
            resolve_protocol(Some("  "), Some("")).unwrap(),
            Protocol::Grpc
        );

        // The shared variable applies to a signal that names none.
        assert_eq!(
            resolve_protocol(None, Some("http/protobuf")).unwrap(),
            Protocol::HttpBinary
        );
        // The signal's own wins over it, in both directions.
        assert_eq!(
            resolve_protocol(Some("grpc"), Some("http/protobuf")).unwrap(),
            Protocol::Grpc
        );
        assert_eq!(
            resolve_protocol(Some("http/protobuf"), Some("grpc")).unwrap(),
            Protocol::HttpBinary
        );
        // Surrounding whitespace is not a different protocol.
        assert_eq!(
            resolve_protocol(Some(" http/protobuf "), None).unwrap(),
            Protocol::HttpBinary
        );

        // `http/json` is a real protocol we cannot speak, and saying so beats
        // silently exporting protobuf; anything else names itself in the error.
        let json = resolve_protocol(Some("http/json"), None)
            .unwrap_err()
            .to_string();
        assert!(json.contains("http/json"), "{json}");
        assert!(json.contains("http/protobuf"), "{json}");
        let unknown = resolve_protocol(None, Some("thrift"))
            .unwrap_err()
            .to_string();
        assert!(unknown.contains("thrift"), "{unknown}");
    }

    /// A protocol we cannot speak degrades to gRPC. It must not be fatal: this
    /// resolves on every `wash` invocation, so a typo in a telemetry variable
    /// would otherwise stop `wash build` from running at all.
    #[test]
    fn an_unusable_protocol_falls_back_rather_than_failing() {
        crate::env_guard::with_vars(
            [
                ("OTEL_EXPORTER_OTLP_PROTOCOL", Some("http/json")),
                ("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", None),
            ],
            || {
                assert_eq!(
                    protocol_from_env(OTEL_EXPORTER_OTLP_TRACES_PROTOCOL),
                    Protocol::Grpc
                );
            },
        );
        crate::env_guard::with_vars(
            [
                ("OTEL_EXPORTER_OTLP_PROTOCOL", None::<&str>),
                ("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", Some("http/protobuf")),
            ],
            || {
                assert_eq!(
                    protocol_from_env(OTEL_EXPORTER_OTLP_TRACES_PROTOCOL),
                    Protocol::HttpBinary
                );
            },
        );
    }

    /// The export timeout the supplied HTTP client carries, resolved the way
    /// the SDK resolves the one it applies itself.
    #[test]
    fn the_signals_timeout_overrides_the_shared_one() {
        crate::env_guard::with_vars(
            [
                ("OTEL_EXPORTER_OTLP_TIMEOUT", Some("3000")),
                ("OTEL_EXPORTER_OTLP_TRACES_TIMEOUT", Some("1500")),
            ],
            || {
                assert_eq!(
                    resolve_timeout(opentelemetry_otlp::OTEL_EXPORTER_OTLP_TRACES_TIMEOUT),
                    Duration::from_millis(1500)
                );
                assert_eq!(
                    resolve_timeout(opentelemetry_otlp::OTEL_EXPORTER_OTLP_LOGS_TIMEOUT),
                    Duration::from_millis(3000)
                );
            },
        );

        crate::env_guard::with_vars(
            [
                ("OTEL_EXPORTER_OTLP_TIMEOUT", None::<&str>),
                ("OTEL_EXPORTER_OTLP_TRACES_TIMEOUT", None),
            ],
            || {
                assert_eq!(
                    resolve_timeout(opentelemetry_otlp::OTEL_EXPORTER_OTLP_TRACES_TIMEOUT),
                    opentelemetry_otlp::OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT
                );
            },
        );
    }

    /// All three signals build over `http/protobuf`. Without a client they
    /// would fail with `NoHttpClient` — `opentelemetry-otlp` supplies one only
    /// behind a `reqwest` feature this build does not take. Only traces are
    /// carried all the way to a collector, by the test above; this covers the
    /// two the SDK would otherwise let fail apart from it.
    #[tokio::test]
    async fn the_http_exporters_have_a_client() {
        crate::env_guard::with_vars(
            [
                ("OTEL_EXPORTER_OTLP_PROTOCOL", Some("http/protobuf")),
                ("OTEL_EXPORTER_OTLP_ENDPOINT", Some("http://localhost:4318")),
            ],
            || {
                build_span_exporter().expect("span exporter over http");
                build_log_exporter().expect("log exporter over http");
                build_metric_exporter().expect("metric exporter over http");
            },
        );
    }

    /// An `http/protobuf` export reaches a collector.
    ///
    /// Building the exporter is not evidence that it works: the SDK exports
    /// from a plain `std::thread` under `futures_executor::block_on`
    /// (`span_processor.rs:316`), so anything the client needs from the ambient
    /// runtime is missing at exactly the moment it sends.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_http_export_reaches_the_collector() {
        use opentelemetry::trace::Tracer as _;
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind collector");
        let port = listener.local_addr().expect("collector addr").port();
        let (sender, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Ok((mut socket, _)) = listener.accept() {
                let mut buffer = [0u8; 1024];
                let read = socket.read(&mut buffer).unwrap_or(0);
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                let _ = sender.send(read);
            }
        });

        let endpoint = format!("http://127.0.0.1:{port}");
        let exporter = crate::env_guard::with_vars(
            [
                ("OTEL_EXPORTER_OTLP_PROTOCOL", Some("http/protobuf")),
                ("OTEL_EXPORTER_OTLP_ENDPOINT", Some(endpoint.as_str())),
            ],
            || build_span_exporter().expect("span exporter over http"),
        );

        let provider = opentelemetry_sdk::trace::TracerProviderBuilder::default()
            .with_batch_exporter(exporter)
            .build();
        provider.tracer("test").in_span("exported", |_| {});
        let _ = provider.force_flush();

        let bytes = received
            .recv_timeout(Duration::from_secs(10))
            .expect("the collector never received an export");
        assert!(bytes > 0, "the collector received an empty request");
    }

    /// `OTEL_<SIGNAL>_EXPORTER` is the specification's per-signal off switch,
    /// and turning one signal off must not disturb the others.
    #[test]
    fn a_signal_can_be_switched_off_without_touching_the_rest() {
        fn selector(value: &str) -> Option<(&str, &str)> {
            configured(OTEL_METRICS_EXPORTER, Some(value))
        }
        let with = |exporter| signal_enabled(false, exporter, base("http://collector:4317"), None);

        // Unset means the default, `otlp`.
        assert_eq!(with(None), (true, None));
        assert_eq!(with(selector("otlp")), (true, None));
        // The one value that turns a signal off — deliberate, so it says nothing.
        assert_eq!(with(selector("none")), (false, None));
        assert_eq!(with(selector("  NONE  ")), (false, None));

        // An exporter we do not implement is off too, but says so: exporting
        // over OTLP anyway would ignore what the operator asked for.
        let (enabled, warning) = with(selector("prometheus"));
        assert!(!enabled);
        let warning = warning.expect("an exporter we cannot provide is reported");
        assert!(warning.contains("prometheus"), "{warning}");
        assert!(warning.contains(OTEL_METRICS_EXPORTER), "{warning}");

        // `none` needs no endpoint to mean what it says.
        assert_eq!(
            signal_enabled(false, selector("none"), None, None),
            (false, None)
        );
    }

    /// An unparseable timeout is reported and ignored rather than silently
    /// dropped — the same class of typo `protocol_from_env` reports.
    #[test]
    fn an_unparseable_timeout_falls_back_to_the_shared_one() {
        crate::env_guard::with_vars(
            [
                ("OTEL_EXPORTER_OTLP_TIMEOUT", Some("3000")),
                ("OTEL_EXPORTER_OTLP_TRACES_TIMEOUT", Some("10s")),
            ],
            || {
                assert_eq!(
                    resolve_timeout(opentelemetry_otlp::OTEL_EXPORTER_OTLP_TRACES_TIMEOUT),
                    Duration::from_millis(3000)
                );
            },
        );
        crate::env_guard::with_vars(
            [
                ("OTEL_EXPORTER_OTLP_TIMEOUT", Some("nonsense")),
                ("OTEL_EXPORTER_OTLP_TRACES_TIMEOUT", None),
            ],
            || {
                assert_eq!(
                    resolve_timeout(opentelemetry_otlp::OTEL_EXPORTER_OTLP_TRACES_TIMEOUT),
                    opentelemetry_otlp::OTEL_EXPORTER_OTLP_TIMEOUT_DEFAULT
                );
            },
        );
    }

    /// A collector behind a private CA is reachable: the specification's own
    /// certificate variable is layered onto the default roots.
    ///
    /// Asserted by building against a real generated CA, because the failure
    /// this guards is silent — an unusable trust store looks exactly like a
    /// working one until an export is attempted.
    #[test]
    fn a_private_ca_is_trusted_for_the_collector() {
        let ca = rcgen::generate_simple_self_signed(vec!["collector.internal".to_string()])
            .expect("generate ca");
        let dir = tempfile::tempdir().expect("tempdir");
        let pem = dir.path().join("collector-ca.pem");
        std::fs::write(&pem, ca.cert.pem()).expect("write ca");

        crate::env_guard::with_vars(
            [(OTEL_EXPORTER_OTLP_CERTIFICATE, Some(pem.as_os_str()))],
            || {
                let path = otlp_certificate_path().expect("the certificate variable is read");
                assert_eq!(path, pem);
                // Layered onto the default roots rather than replacing them, so
                // naming a private CA does not stop the public ones working.
                crate::host::http_client::ClientTlsOptions {
                    extra_ca_paths: vec![path],
                    ..Default::default()
                }
                .build()
                .expect("the private CA builds a usable trust store");
            },
        );

        // A bundle we cannot read is reported and skipped rather than fatal —
        // asserted on the builder, since `otlp_trust_roots` caches its answer
        // for the process and cannot be exercised twice.
        assert!(
            crate::host::http_client::ClientTlsOptions {
                extra_ca_paths: vec![dir.path().join("absent.pem")],
                ..Default::default()
            }
            .build()
            .is_err(),
            "an unreadable bundle is an error the caller can fall back from"
        );

        // Unset leaves the host's own default roots in place.
        crate::env_guard::with_vars([(OTEL_EXPORTER_OTLP_CERTIFICATE, None::<&str>)], || {
            assert!(otlp_certificate_path().is_none());
        });
    }

    /// A client certificate needs its key. Half a pair cannot authenticate, and
    /// connecting anonymously anyway would look like working mTLS right up to
    /// the point the collector refused it.
    #[test]
    fn client_authentication_needs_both_halves() {
        let cert = "/tmp/otlp-client.pem";
        let key = "/tmp/otlp-client-key.pem";

        crate::env_guard::with_vars(
            [
                (OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE, Some(cert)),
                (OTEL_EXPORTER_OTLP_CLIENT_KEY, Some(key)),
            ],
            || {
                let (certificate, key_path) =
                    resolve_client_identity().expect("both halves are an identity");
                assert_eq!(certificate, std::path::Path::new(cert));
                assert_eq!(key_path, std::path::Path::new(key));
            },
        );

        // Either half alone is a misconfiguration, not an identity.
        crate::env_guard::with_vars(
            [
                (OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE, Some(cert)),
                (OTEL_EXPORTER_OTLP_CLIENT_KEY, None),
            ],
            || assert!(resolve_client_identity().is_none()),
        );
        crate::env_guard::with_vars(
            [
                (OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE, None),
                (OTEL_EXPORTER_OTLP_CLIENT_KEY, Some(key)),
            ],
            || assert!(resolve_client_identity().is_none()),
        );
        crate::env_guard::with_vars(
            [
                (OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE, None::<&str>),
                (OTEL_EXPORTER_OTLP_CLIENT_KEY, None),
            ],
            || assert!(resolve_client_identity().is_none()),
        );
    }

    /// A real client certificate and key build a usable rustls configuration.
    #[test]
    fn a_client_certificate_builds_a_usable_config() {
        let identity = rcgen::generate_simple_self_signed(vec!["wash".to_string()])
            .expect("generate client identity");
        let dir = tempfile::tempdir().expect("tempdir");
        let cert = dir.path().join("client.pem");
        let key = dir.path().join("client-key.pem");
        std::fs::write(&cert, identity.cert.pem()).expect("write cert");
        std::fs::write(&key, identity.signing_key.serialize_pem()).expect("write key");

        otlp_client_config(None, Some(&(cert, key)))
            .expect("a client certificate builds a usable config");
    }

    /// An operator who asks for transport security and does not get it is told.
    ///
    /// `OTEL_EXPORTER_OTLP_INSECURE` is a specification variable that neither
    /// this runtime nor `opentelemetry-otlp` can act on — the endpoint's scheme
    /// decides — so `false` against an `http://` collector silently ships
    /// telemetry in cleartext. That is the one direction worth interrupting for.
    #[test]
    fn asking_for_security_and_not_getting_it_is_reported() {
        assert!(insecure_disagreement(
            Some("false"),
            "http://collector:4317"
        ));
        assert!(insecure_disagreement(
            Some(" FALSE "),
            "http://collector:4317"
        ));

        // Already encrypted: the endpoint gives what was asked for.
        assert!(!insecure_disagreement(
            Some("false"),
            "https://collector:4317"
        ));
        // The common, harmless case — a plaintext in-cluster collector with the
        // variable at its default. Warning here would cry wolf.
        assert!(!insecure_disagreement(
            Some("true"),
            "http://collector:4317"
        ));
        assert!(!insecure_disagreement(None, "http://collector:4317"));
    }

    /// The budget bounds the flush; it is not spent waiting on one. A process
    /// that configured no exporter — every `wash` invocation with no OTLP
    /// endpoint set — has nothing to hand over and leaves on a signal at once.
    #[tokio::test]
    async fn flushing_without_an_exporter_does_not_spend_the_budget() {
        let started = std::time::Instant::now();
        assert!(flush_within(FLUSH_BUDGET).await);
        assert!(started.elapsed() < FLUSH_BUDGET);
    }

    /// An embedder composes the layers into a `Registry` of its own, and gets
    /// working instruments out of the global meter — the whole point of
    /// separating this from the subscriber install.
    ///
    /// No endpoint is configured under `cargo test`, so nothing is enabled and
    /// both layers are `None`; what this pins is that composing them is
    /// possible at all without a subscriber being installed underneath.
    #[test]
    fn providers_install_without_touching_the_global_subscriber() {
        use tracing_subscriber::layer::SubscriberExt as _;

        // On a machine that exports OTLP endpoints for its own use, installing
        // here would leave `flush` a live batch processor with no collector to
        // drain into, and the flush test above measures how long that takes.
        if otel_enabled() {
            return;
        }

        let providers = install_providers().expect("providers install");
        let subscriber = tracing_subscriber::Registry::default()
            .with(providers.logs_layer())
            .with(providers.tracing_layer());

        // Scoped rather than global: a unit test must not claim the process's
        // one global subscriber slot out from under any other test.
        tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!("embedded").in_scope(|| tracing::info!("recorded"));
        });

        let histogram = opentelemetry::global::meter("wash-runtime")
            .f64_histogram("test.instrument")
            .build();
        histogram.record(1.0, &[]);
    }
}
