//! What this runtime measures about its guests ([`meters`]), and where the
//! telemetry goes ([`otel`]).

mod meters;
// `pub(crate)` for the exporter builders the `wasi-otel` plugin shares. Naming
// them through the module rather than re-exporting keeps that path from
// reading as unused in a build without that feature.
pub(crate) mod otel;

pub use meters::{
    FuelConsumptionMeter, GuestMeter, InvocationMeter, MeterKind, Meters, WorkloadIdentity,
};
pub use otel::{FLUSH_BUDGET, Providers, flush, flush_within, install_providers};

pub use otel::log_configuration;
use otel::otel_enabled;

use anyhow::Context as _;
use tracing::Level;
use tracing_subscriber::{
    EnvFilter, Layer, Registry, filter::Directive, layer::SubscriberExt, util::SubscriberInitExt,
};

/// Initialize observability, setting up console & OpenTelemetry layers.
///
/// Returns a shutdown function that should be called on process exit to flush
/// any remaining spans/logs. It is [`flush`], which runs at most once — so a
/// process that already flushed on its way out of a signal handler does not
/// shut the providers down twice.
///
/// This claims the process's global subscriber. An embedder that has one of its
/// own calls [`install_providers`] and adds [`Providers::logs_layer`] and
/// [`Providers::tracing_layer`] to its own `Registry` instead.
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

    // Whatever the environment asked for, this process still gets its console
    // logging: telemetry that cannot be set up is worth a warning, not an exit.
    // Exporters are built before the subscriber is installed, so a failure here
    // would otherwise end the process with nothing able to say why.
    let providers = if otel_enabled() {
        match install_providers() {
            Ok(providers) => Some(providers),
            Err(e) => {
                Registry::default()
                    .with(fmt_layer)
                    .try_init()
                    .context("failed to install the tracing subscriber")?;
                tracing::warn!("failed to set up OTLP export: {e:#}; continuing without it");
                log_configuration();
                return Ok(Box::new(flush));
            }
        }
    } else {
        None
    };

    let Some(providers) = providers else {
        Registry::default()
            .with(fmt_layer)
            .try_init()
            .context("failed to install the tracing subscriber")?;
        log_configuration();

        // Nothing to flush: `flush` finds no registered shutdown and returns.
        return Ok(Box::new(flush));
    };
    // The filters the `wash` CLI wants on the exported signals; the layers
    // themselves are unfiltered so an embedder can choose differently.
    let otel_logs_layer = providers
        .logs_layer()
        .map(|layer| layer.with_filter(EnvFilter::new(log_level.as_str())));
    let otel_tracer_layer = providers
        .tracing_layer()
        .map(|layer| layer.with_filter(EnvFilter::new(log_level.as_str())));

    // `try_init` rather than `init`: `init` is `try_init().expect(..)`, and a
    // second install is a caller mistake worth reporting rather than a panic.
    Registry::default()
        .with(fmt_layer)
        .with(otel_logs_layer)
        .with(otel_tracer_layer)
        .try_init()
        .context("failed to install the tracing subscriber")?;
    log_configuration();

    Ok(Box::new(flush))
}

/// Helper function to reduce duplication and code size for parsing directives
fn directive(directive: impl AsRef<str>) -> anyhow::Result<Directive> {
    directive
        .as_ref()
        .parse()
        .with_context(|| format!("failed to parse filter: {}", directive.as_ref()))
}
