use std::env;
use std::io::{IsTerminal, StderrLock, Write};

use anyhow::Context;
use once_cell::sync::OnceCell;
#[cfg(feature = "otel")]
use opentelemetry_otlp::WithExportConfig;
use tracing::{Event, Subscriber};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::{DefaultFields, Format, Full, Json, JsonFields, Writer};
use tracing_subscriber::fmt::time::SystemTime;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::{Layered, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{EnvFilter, Layer, Registry};
use wasmcloud_core::logging::Level;
use wasmcloud_core::OtelConfig;

struct LockedWriter<'a> {
    stderr: StderrLock<'a>,
}

impl<'a> LockedWriter<'a> {
    fn new() -> Self {
        LockedWriter {
            stderr: STDERR.get_or_init(std::io::stderr).lock(),
        }
    }
}

impl<'a> Write for LockedWriter<'a> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stderr.write(buf)
    }

    /// DIRTY HACK: when flushing, write a carriage return so the output is clean and then flush
    fn flush(&mut self) -> std::io::Result<()> {
        self.stderr.write_all(&[13])?;
        self.stderr.flush()
    }
}

impl<'a> Drop for LockedWriter<'a> {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

static STDERR: OnceCell<std::io::Stderr> = OnceCell::new();

#[cfg(feature = "otel")]
static LOG_PROVIDER: OnceCell<opentelemetry_sdk::logs::LoggerProvider> = OnceCell::new();

/// A struct that allows us to dynamically choose JSON formatting without using dynamic dispatch.
/// This is just so we avoid any sort of possible slow down in logging code
enum JsonOrNot {
    Not(Format<Full, SystemTime>),
    Json(Format<Json, SystemTime>),
}

impl<S, N> FormatEvent<S, N> for JsonOrNot
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        match self {
            JsonOrNot::Not(f) => f.format_event(ctx, writer, event),
            JsonOrNot::Json(f) => f.format_event(ctx, writer, event),
        }
    }
}

/// Configures a global tracing subscriber, which includes:
/// - A level filter, which forms the base and applies to all other layers
/// - A local logging layer, which is either plaintext or structured (JSON)
///
/// # Errors
///
/// This will return an error if the function has already been called, or if we fail to create any
/// of the layers
#[cfg(not(feature = "otel"))]
pub fn configure_tracing(
    _: &str,
    _: &OtelConfig,
    structured_logging_enabled: bool,
    log_level_override: Option<&Level>,
) -> anyhow::Result<()> {
    STDERR
        .set(std::io::stderr())
        .map_err(|_| anyhow::anyhow!("stderr already initialized"))?;

    let base_reg = tracing_subscriber::Registry::default();
    let level_filter = get_level_filter(log_level_override);

    let res = if structured_logging_enabled {
        let log_layer = get_json_log_layer()?;
        let layered = base_reg.with(level_filter).with(log_layer);
        tracing::subscriber::set_global_default(layered)
    } else {
        let log_layer = get_plaintext_log_layer()?;
        let layered = base_reg.with(level_filter).with(log_layer);
        tracing::subscriber::set_global_default(layered)
    };

    res.map_err(|e| anyhow::anyhow!(e).context("Logger was already created"))
}

/// Configures a global tracing subscriber, which includes:
/// - A level filter, which forms the base and applies to all other layers
/// - OTEL tracing and logging layers, if OTEL configuration is provided
/// - A local logging layer, which is either plaintext or structured (JSON)
///
/// # Errors
///
/// This will return an error if the function has already been called, or if we fail to create any
/// of the layers
#[cfg(feature = "otel")]
pub fn configure_tracing(
    service_name: &str,
    otel_config: &OtelConfig,
    use_structured_logging: bool,
    log_level_override: Option<&Level>,
) -> anyhow::Result<()> {
    STDERR
        .set(std::io::stderr())
        .map_err(|_| anyhow::anyhow!("stderr already initialized"))?;

    let enable_traces = otel_config
        .enable_traces
        .unwrap_or(otel_config.enable_observability);

    let enable_logs = otel_config
        .enable_logs
        .unwrap_or(otel_config.enable_observability);

    let base_reg = tracing_subscriber::Registry::default();

    let level_filter = get_level_filter(log_level_override);

    let normalized_service_name = service_name.to_string();

    let traces_endpoint = otel_config
        .traces_endpoint
        .clone()
        .or_else(|| otel_config.observability_endpoint.clone());

    let logs_endpoint = otel_config
        .logs_endpoint
        .clone()
        .or_else(|| otel_config.observability_endpoint.clone());

    // NOTE: this logic would be simpler if we could conditionally/imperatively construct and add
    // layers, but due to the dynamic types, this is not possible
    let res = match (enable_traces, enable_logs, use_structured_logging) {
        (true, true, true) => {
            let layered = base_reg
                .with(level_filter)
                .with(get_json_log_layer()?)
                .with(get_otel_tracing_layer(
                    &traces_endpoint,
                    normalized_service_name.clone(),
                )?)
                .with(get_otel_logging_layer(
                    &logs_endpoint,
                    normalized_service_name,
                )?);
            tracing::subscriber::set_global_default(layered)
        }
        (true, true, false) => {
            let layered = base_reg
                .with(level_filter)
                .with(get_plaintext_log_layer()?)
                .with(get_otel_tracing_layer(
                    &traces_endpoint,
                    normalized_service_name.clone(),
                )?)
                .with(get_otel_logging_layer(
                    &logs_endpoint,
                    normalized_service_name,
                )?);
            tracing::subscriber::set_global_default(layered)
        }
        (true, false, true) => {
            let layered = base_reg
                .with(level_filter)
                .with(get_json_log_layer()?)
                .with(get_otel_tracing_layer(
                    &traces_endpoint,
                    normalized_service_name.clone(),
                )?);
            tracing::subscriber::set_global_default(layered)
        }
        (true, false, false) => {
            let layered = base_reg
                .with(level_filter)
                .with(get_plaintext_log_layer()?)
                .with(get_otel_tracing_layer(
                    &traces_endpoint,
                    normalized_service_name.clone(),
                )?);
            tracing::subscriber::set_global_default(layered)
        }
        (false, true, true) => {
            let layered = base_reg
                .with(level_filter)
                .with(get_json_log_layer()?)
                .with(get_otel_logging_layer(
                    &logs_endpoint,
                    normalized_service_name,
                )?);
            tracing::subscriber::set_global_default(layered)
        }
        (false, true, false) => {
            let layered = base_reg
                .with(level_filter)
                .with(get_plaintext_log_layer()?)
                .with(get_otel_logging_layer(
                    &logs_endpoint,
                    normalized_service_name,
                )?);
            tracing::subscriber::set_global_default(layered)
        }
        (false, false, true) => {
            let layered = base_reg.with(level_filter).with(get_json_log_layer()?);
            tracing::subscriber::set_global_default(layered)
        }
        (false, false, false) => {
            let layered = base_reg.with(level_filter).with(get_plaintext_log_layer()?);
            tracing::subscriber::set_global_default(layered)
        }
    };

    res.map_err(|e| anyhow::anyhow!(e).context("Logger/tracer was already created"))
}

#[cfg(feature = "otel")]
fn get_otel_tracing_layer<S>(
    exporter_endpoint: &Option<String>,
    service_name: String,
) -> anyhow::Result<impl Layer<S>>
where
    S: Subscriber,
    S: for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let mut builder = opentelemetry_otlp::new_exporter()
        .http()
        .with_protocol(opentelemetry_otlp::Protocol::HttpBinary);

    if let Some(ref endpoint) = exporter_endpoint {
        builder = builder.with_endpoint(endpoint);
    }

    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(builder)
        .with_trace_config(
            opentelemetry_sdk::trace::config()
                .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
                .with_id_generator(opentelemetry_sdk::trace::RandomIdGenerator::default())
                .with_max_events_per_span(64)
                .with_max_attributes_per_span(16)
                .with_max_events_per_span(16)
                .with_resource(opentelemetry_sdk::Resource::new(vec![
                    opentelemetry::KeyValue::new("service.name", service_name),
                ])),
        )
        .install_batch(opentelemetry_sdk::runtime::Tokio)
        .context("failed to create OTEL tracer")?;

    Ok(tracing_opentelemetry::layer().with_tracer(tracer))
}

#[cfg(feature = "otel")]
fn get_otel_logging_layer<S>(
    exporter_endpoint: &Option<String>,
    service_name: String,
) -> anyhow::Result<impl Layer<S>>
where
    S: Subscriber,
    S: for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let mut builder = opentelemetry_otlp::HttpExporterBuilder::default();

    if let Some(ref endpoint) = exporter_endpoint {
        builder = builder.with_endpoint(endpoint);
    }

    let exporter = opentelemetry_otlp::LogExporterBuilder::Http(builder)
        .build_log_exporter()
        .context("failed to create OTEL log exporter")?;

    let log_provider = opentelemetry_sdk::logs::LoggerProvider::builder()
        .with_config(opentelemetry_sdk::logs::Config::default().with_resource(
            opentelemetry_sdk::Resource::new(vec![opentelemetry::KeyValue::new(
                "service.name",
                service_name,
            )]),
        ))
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .build();

    // Prevent the exporter/provider from being dropped
    LOG_PROVIDER
        .set(log_provider)
        .map_err(|_| anyhow::anyhow!("Logger provider already initialized"))?;

    let log_layer = opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
        LOG_PROVIDER.get().unwrap(),
    );

    Ok(log_layer)
}

fn get_plaintext_log_layer<S>() -> anyhow::Result<impl Layer<S>>
where
    S: Subscriber,
    S: for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let stderr = STDERR.get().context("stderr not initialized")?;
    Ok(tracing_subscriber::fmt::layer()
        .with_writer(LockedWriter::new)
        .with_ansi(stderr.is_terminal())
        .event_format(JsonOrNot::Not(Format::default()))
        .fmt_fields(DefaultFields::new()))
}

fn get_json_log_layer() -> anyhow::Result<impl Layer<Layered<EnvFilter, Registry>>> {
    let stderr = STDERR.get().context("stderr not initialized")?;
    Ok(tracing_subscriber::fmt::layer()
        .with_writer(LockedWriter::new)
        .with_ansi(stderr.is_terminal())
        .event_format(JsonOrNot::Json(Format::default().json()))
        .fmt_fields(JsonFields::new()))
}

fn get_level_filter(log_level_override: Option<&Level>) -> EnvFilter {
    if let Some(log_level) = log_level_override {
        let level = wasi_level_to_tracing_level(log_level);
        // SAFETY: We can unwrap here because we control all inputs
        let mut filter = EnvFilter::builder()
            .with_default_directive(level.into())
            .parse("")
            .unwrap()
            .add_directive("async_nats=info".parse().unwrap())
            .add_directive("cranelift_codegen=warn".parse().unwrap())
            .add_directive("hyper=info".parse().unwrap())
            .add_directive("oci_distribution=info".parse().unwrap());

        // Allow RUST_LOG to override the other directives
        if let Ok(rust_log) = env::var("RUST_LOG") {
            match rust_log
                .split(',')
                .map(str::parse)
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(directives) => {
                    for directive in directives {
                        filter = filter.add_directive(directive);
                    }
                }
                Err(err) => {
                    eprintln!("ERROR: Ignoring invalid RUST_LOG directive: {err}");
                }
            }
        }

        filter
    } else {
        EnvFilter::default().add_directive(LevelFilter::INFO.into())
    }
}

fn wasi_level_to_tracing_level(level: &Level) -> LevelFilter {
    match level {
        Level::Error | Level::Critical => LevelFilter::ERROR,
        Level::Warn => LevelFilter::WARN,
        Level::Info => LevelFilter::INFO,
        Level::Debug => LevelFilter::DEBUG,
        Level::Trace => LevelFilter::TRACE,
    }
}
