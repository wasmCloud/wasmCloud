use std::env;
use std::fs::File;
use std::io::{BufWriter, IsTerminal};
use std::path::Path;
#[cfg(feature = "otel")]
use std::sync::Arc;

use anyhow::Context as _;
#[cfg(feature = "otel")]
use opentelemetry_otlp::{LogExporterBuilder, SpanExporterBuilder, WithExportConfig};
use tracing::{Event, Subscriber};
use tracing_flame::FlameLayer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::{DefaultFields, Format, Full, Json, JsonFields, Writer};
use tracing_subscriber::fmt::time::SystemTime;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::EnvFilter;
use wasmcloud_core::logging::Level;
use wasmcloud_core::OtelConfig;
#[cfg(feature = "otel")]
use wasmcloud_core::OtelProtocol;

#[cfg(feature = "otel")]
static LOG_PROVIDER: once_cell::sync::OnceCell<opentelemetry_sdk::logs::LoggerProvider> =
    once_cell::sync::OnceCell::new();

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

pub struct FlushGuard {
    _stderr: tracing_appender::non_blocking::WorkerGuard,
    _flame: Option<tracing_flame::FlushGuard<BufWriter<File>>>,
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
    use_structured_logging: bool,
    flame_graph: Option<impl AsRef<Path>>,
    log_level_override: Option<&Level>,
) -> anyhow::Result<FlushGuard> {
    let flame = flame_graph.map(FlameLayer::with_file).transpose()?;
    let (flame, flame_guard) = flame.map(|(l, g)| (Some(l), Some(g))).unwrap_or_default();
    let reg = tracing_subscriber::Registry::default()
        .with(get_level_filter(log_level_override))
        .with(flame);
    let stderr = std::io::stderr();
    let ansi = stderr.is_terminal();
    let (stderr, stderr_guard) = tracing_appender::non_blocking(stderr);
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(stderr)
        .with_ansi(ansi);
    if use_structured_logging {
        tracing::subscriber::set_global_default(
            reg.with(
                fmt.event_format(JsonOrNot::Json(Format::default().json()))
                    .fmt_fields(JsonFields::new()),
            ),
        )
    } else {
        tracing::subscriber::set_global_default(
            reg.with(
                fmt.event_format(JsonOrNot::Not(Format::default()))
                    .fmt_fields(DefaultFields::new()),
            ),
        )
    }
    .context("logger already configured")?;
    Ok(FlushGuard {
        _stderr: stderr_guard,
        _flame: flame_guard,
    })
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
    flame_graph: Option<impl AsRef<Path>>,
    log_level_override: Option<&Level>,
) -> anyhow::Result<FlushGuard> {
    let service_name = Arc::from(service_name);

    let traces = otel_config
        .traces_enabled()
        .then(|| get_otel_tracing_layer(Arc::clone(&service_name), otel_config))
        .transpose()?;
    let logs = otel_config
        .logs_enabled()
        .then(|| get_otel_logging_layer(Arc::clone(&service_name), otel_config))
        .transpose()?;
    let flame = flame_graph.map(FlameLayer::with_file).transpose()?;
    let (flame, flame_guard) = flame.map(|(l, g)| (Some(l), Some(g))).unwrap_or_default();
    let reg = tracing_subscriber::Registry::default()
        .with(get_level_filter(log_level_override))
        .with(traces)
        .with(logs)
        .with(flame);
    let stderr = std::io::stderr();
    let ansi = stderr.is_terminal();
    let (stderr, stderr_guard) = tracing_appender::non_blocking(stderr);
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(stderr)
        .with_ansi(ansi);
    if use_structured_logging {
        tracing::subscriber::set_global_default(
            reg.with(
                fmt.event_format(JsonOrNot::Json(Format::default().json()))
                    .fmt_fields(JsonFields::new()),
            ),
        )
    } else {
        tracing::subscriber::set_global_default(
            reg.with(
                fmt.event_format(JsonOrNot::Not(Format::default()))
                    .fmt_fields(DefaultFields::new()),
            ),
        )
    }
    .context("logger/tracer already configured")?;
    Ok(FlushGuard {
        _stderr: stderr_guard,
        _flame: flame_guard,
    })
}

#[cfg(feature = "otel")]
fn get_otel_tracing_layer<S>(
    service_name: Arc<str>,
    otel_config: &OtelConfig,
) -> anyhow::Result<impl tracing_subscriber::Layer<S>>
where
    S: Subscriber,
    S: for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let builder: SpanExporterBuilder = match otel_config.protocol {
        OtelProtocol::Http => opentelemetry_otlp::new_exporter()
            .http()
            .with_endpoint(otel_config.traces_endpoint())
            .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
            .into(),
        OtelProtocol::Grpc => opentelemetry_otlp::new_exporter()
            .tonic()
            .with_endpoint(otel_config.traces_endpoint())
            .into(),
    };

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
    service_name: Arc<str>,
    otel_config: &OtelConfig,
) -> anyhow::Result<impl tracing_subscriber::Layer<S>>
where
    S: Subscriber,
    S: for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let builder: LogExporterBuilder = match otel_config.protocol {
        OtelProtocol::Http => opentelemetry_otlp::new_exporter()
            .http()
            .with_endpoint(otel_config.logs_endpoint())
            .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
            .into(),
        OtelProtocol::Grpc => opentelemetry_otlp::new_exporter()
            .tonic()
            .with_endpoint(otel_config.logs_endpoint())
            .into(),
    };

    let log_provider = opentelemetry_otlp::new_pipeline()
        .logging()
        .with_log_config(opentelemetry_sdk::logs::Config::default().with_resource(
            opentelemetry_sdk::Resource::new(vec![opentelemetry::KeyValue::new(
                "service.name",
                service_name,
            )]),
        ))
        .with_exporter(builder)
        .install_batch(opentelemetry_sdk::runtime::Tokio)
        .context("failed to create OTEL logger provider")?;

    // Prevent the exporter/provider from being dropped
    LOG_PROVIDER
        .set(log_provider)
        .map_err(|_| anyhow::anyhow!("Logger provider already initialized"))?;

    let log_layer = opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
        LOG_PROVIDER.get().unwrap(),
    );

    Ok(log_layer)
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
