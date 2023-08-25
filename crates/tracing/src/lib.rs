pub mod context;

use std::io::{IsTerminal, StderrLock, Write};

use anyhow::Context;
use once_cell::sync::OnceCell;
#[cfg(feature = "otel")]
use opentelemetry::sdk::{
    trace::{self, RandomIdGenerator, Sampler, Tracer},
    Resource,
};
#[cfg(feature = "otel")]
use opentelemetry::trace::TraceError;
#[cfg(feature = "otel")]
use opentelemetry_otlp::{Protocol, WithExportConfig};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::{
    format::{DefaultFields, Format, Full, Json, JsonFields, Writer},
    time::SystemTime,
    FmtContext, FormatEvent, FormatFields,
};
use tracing_subscriber::{
    filter::LevelFilter,
    layer::{Layered, SubscriberExt},
    registry::LookupSpan,
    EnvFilter, Layer, Registry,
};

use wasmcloud_core::{logging::Level, OtelConfig};

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
const TRACING_PATH: &str = "/v1/traces";

#[cfg(feature = "otel")]
const DEFAULT_TRACING_ENDPOINT: &str = "http://localhost:55681/v1/traces";

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

#[cfg(not(feature = "otel"))]
pub fn configure_tracing(
    _: String,
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
        let log_layer = get_default_log_layer()?;
        let layered = base_reg.with(level_filter).with(log_layer);
        tracing::subscriber::set_global_default(layered)
    };

    res.map_err(|e| anyhow::anyhow!(e).context("Logger was already created"))
}

#[cfg(feature = "otel")]
pub fn configure_tracing(
    service_name: String,
    otel_config: &OtelConfig,
    structured_logging_enabled: bool,
    log_level_override: Option<&Level>,
) -> anyhow::Result<()> {
    STDERR
        .set(std::io::stderr())
        .map_err(|_| anyhow::anyhow!("stderr already initialized"))?;

    let base_reg = tracing_subscriber::Registry::default();
    let level_filter = get_level_filter(log_level_override);

    let exporter = otel_config
        .traces_exporter
        .as_ref()
        .map(|s| s.to_ascii_lowercase());

    let maybe_tracer = match exporter {
        Some(exporter) if exporter == "otlp" => {
            let tracing_endpoint = otel_config.exporter_otlp_endpoint.clone();

            match tracing_endpoint {
                Some(endpoint) => Some(get_tracer(endpoint, service_name)),
                None => {
                    eprintln!(
                        "OTEL exporter endpoint not set, defaulting to '{DEFAULT_TRACING_ENDPOINT}'"
                    );
                    Some(get_tracer(
                        DEFAULT_TRACING_ENDPOINT.to_string(),
                        service_name,
                    ))
                }
            }
        }
        Some(exporter) => {
            eprintln!("unsupported OTEL exporter: '{}'", exporter);
            None
        }
        None => None,
    };

    let res = match (maybe_tracer, structured_logging_enabled) {
        (Some(Ok(t)), true) => {
            let log_layer = get_json_log_layer()?;
            let tracing_layer = tracing_opentelemetry::layer().with_tracer(t);
            let layered = base_reg
                .with(level_filter)
                .with(log_layer)
                .with(tracing_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (Some(Ok(t)), false) => {
            let log_layer = get_default_log_layer()?;
            let tracing_layer = tracing_opentelemetry::layer().with_tracer(t);
            let layered = base_reg
                .with(level_filter)
                .with(log_layer)
                .with(tracing_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (Some(Err(err)), true) => {
            eprintln!("Unable to configure OTEL tracing, defaulting to logging only: {err:?}");
            let log_layer = get_json_log_layer()?;
            let layered = base_reg.with(level_filter).with(log_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (Some(Err(err)), false) => {
            eprintln!("Unable to configure OTEL tracing, defaulting to logging only: {err:?}");
            let log_layer = get_default_log_layer()?;
            let layered = base_reg.with(level_filter).with(log_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (None, true) => {
            let log_layer = get_json_log_layer()?;
            let layered = base_reg.with(level_filter).with(log_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (None, false) => {
            let log_layer = get_default_log_layer()?;
            let layered = base_reg.with(level_filter).with(log_layer);
            tracing::subscriber::set_global_default(layered)
        }
    };

    res.map_err(|e| anyhow::anyhow!(e).context("Logger/tracer was already created"))
}

#[cfg(feature = "otel")]
fn get_tracer(mut tracing_endpoint: String, service_name: String) -> Result<Tracer, TraceError> {
    if !tracing_endpoint.ends_with(TRACING_PATH) {
        tracing_endpoint.push_str(TRACING_PATH)
    };

    opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(
            opentelemetry_otlp::new_exporter()
                .http()
                .with_endpoint(tracing_endpoint)
                .with_protocol(Protocol::HttpBinary),
        )
        .with_trace_config(
            trace::config()
                .with_sampler(Sampler::AlwaysOn)
                .with_id_generator(RandomIdGenerator::default())
                .with_max_events_per_span(64)
                .with_max_attributes_per_span(16)
                .with_max_events_per_span(16)
                .with_resource(Resource::new(vec![opentelemetry::KeyValue::new(
                    "service.name",
                    service_name,
                )])),
        )
        .install_batch(opentelemetry::runtime::Tokio)
}

fn get_default_log_layer() -> anyhow::Result<impl Layer<Layered<EnvFilter, Registry>>> {
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
        // NOTE(thomastaylor312): Technically we should just use the plain level filter, but we are
        // cheating so we don't have to mix dynamic filter types.
        // SAFETY: We can unwrap here because we control all inputs
        EnvFilter::builder()
            .with_default_directive(level.into())
            .parse("")
            .unwrap()
    } else {
        EnvFilter::default().add_directive(LevelFilter::INFO.into())
    }
}

fn wasi_level_to_tracing_level(level: &Level) -> LevelFilter {
    match level {
        Level::Error => LevelFilter::ERROR,
        Level::Critical => LevelFilter::ERROR,
        Level::Warn => LevelFilter::WARN,
        Level::Info => LevelFilter::INFO,
        Level::Debug => LevelFilter::DEBUG,
        Level::Trace => LevelFilter::TRACE,
    }
}
