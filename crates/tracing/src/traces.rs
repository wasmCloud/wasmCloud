use std::env;
use std::fs::File;
use std::io::{BufWriter, IsTerminal};
use std::path::Path;
#[cfg(feature = "otel")]
use std::sync::Arc;

#[cfg(feature = "otel")]
use anyhow::Context as _;
#[cfg(feature = "otel")]
use opentelemetry_otlp::WithExportConfig;
use tracing::{Event, Subscriber};
use tracing_flame::FlameLayer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::{DefaultFields, Format, Full, Json, JsonFields, Writer};
use tracing_subscriber::fmt::time::SystemTime;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::EnvFilter;
#[cfg(feature = "otel")]
use tracing_subscriber::Layer;
use wasmcloud_core::logging::Level;
use wasmcloud_core::OtelConfig;
#[cfg(feature = "otel")]
use wasmcloud_core::OtelProtocol;

#[cfg(feature = "otel")]
static LOG_PROVIDER: once_cell::sync::OnceCell<opentelemetry_sdk::logs::SdkLoggerProvider> =
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

/// This guard prevents early `drop()`ing of the tracing related internal data structures
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
) -> anyhow::Result<(tracing::Dispatch, FlushGuard)> {
    let flame = flame_graph.map(FlameLayer::with_file).transpose()?;
    let (flame, flame_guard) = flame.map(|(l, g)| (Some(l), Some(g))).unwrap_or_default();
    let reg = tracing_subscriber::Registry::default()
        .with(get_log_level_filter(log_level_override))
        .with(flame);
    let stderr = std::io::stderr();
    let ansi = stderr.is_terminal();
    let (stderr, stderr_guard) = tracing_appender::non_blocking(stderr);
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(stderr)
        .with_ansi(ansi);

    let dispatch = if use_structured_logging {
        reg.with(
            fmt.event_format(JsonOrNot::Json(Format::default().json()))
                .fmt_fields(JsonFields::new()),
        )
        .into()
    } else {
        reg.with(
            fmt.event_format(JsonOrNot::Not(Format::default()))
                .fmt_fields(DefaultFields::new()),
        )
        .into()
    };

    Ok((
        dispatch,
        FlushGuard {
            _stderr: stderr_guard,
            _flame: flame_guard,
        },
    ))
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
    trace_level_override: Option<&Level>,
) -> anyhow::Result<(tracing::Dispatch, FlushGuard)> {
    let service_name = Arc::from(service_name);

    let log_level_filter = get_log_level_filter(log_level_override);
    let traces = otel_config
        .traces_enabled()
        .then(|| {
            get_otel_tracing_layer(
                Arc::clone(&service_name),
                otel_config,
                get_trace_level_filter(trace_level_override),
            )
        })
        .transpose()?;
    let logs = otel_config
        .logs_enabled()
        .then(|| get_otel_logging_layer(Arc::clone(&service_name), otel_config, log_level_override))
        .transpose()?;
    let flame = flame_graph.map(FlameLayer::with_file).transpose()?;
    let (flame, flame_guard) = flame
        .map(|(l, g)| {
            (
                Some(l.with_filter(get_trace_level_filter(trace_level_override))),
                Some(g),
            )
        })
        .unwrap_or_default();
    let registry = tracing_subscriber::Registry::default()
        .with(get_log_level_filter(log_level_override))
        .with(traces)
        .with(logs)
        .with(flame);
    let stderr = std::io::stderr();
    let ansi = stderr.is_terminal();
    let (stderr, stderr_guard) = tracing_appender::non_blocking(stderr);
    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(stderr)
        .with_ansi(ansi);

    let dispatch = if use_structured_logging {
        registry
            .with(
                fmt.event_format(JsonOrNot::Json(Format::default().json()))
                    .fmt_fields(JsonFields::new())
                    .with_filter(log_level_filter),
            )
            .into()
    } else {
        registry
            .with(
                fmt.event_format(JsonOrNot::Not(Format::default()))
                    .fmt_fields(DefaultFields::new())
                    .with_filter(log_level_filter),
            )
            .into()
    };

    Ok((
        dispatch,
        FlushGuard {
            _stderr: stderr_guard,
            _flame: flame_guard,
        },
    ))
}

#[cfg(feature = "otel")]
fn get_otel_tracing_layer<S>(
    service_name: Arc<str>,
    otel_config: &OtelConfig,
    trace_level_filter: EnvFilter,
) -> anyhow::Result<impl tracing_subscriber::Layer<S>>
where
    S: Subscriber,
    S: for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithHttpConfig;
    use opentelemetry_sdk::trace::{BatchConfigBuilder, Sampler};
    use tracing_opentelemetry::OpenTelemetryLayer;

    let exporter = match otel_config.protocol {
        OtelProtocol::Http => {
            let client = crate::get_http_client(otel_config)
                .context("failed to get an http client for otel tracing exporter")?;
            opentelemetry_otlp::SpanExporter::builder()
                .with_http()
                .with_http_client(client)
                .with_endpoint(otel_config.traces_endpoint())
                .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
                .build()
                .context("failed to build OTEL span exporter")?
        }
        OtelProtocol::Grpc => {
            // TODO(joonas): Configure tonic::transport::ClientTlsConfig via .with_tls_config(...), passing in additional certificates.
            opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_endpoint(otel_config.traces_endpoint())
                .build()
                .context("failed to build OTEL span exporter")?
        }
    };

    // NOTE(thomastaylor312): This is copied and modified from the opentelemetry-sdk crate. We
    // currently need this because providers map config back into the vars needed to configure the
    // SDK. When we update providers to be managed externally and remove host-managed ones, we can
    // remove this. But for now we need to parse all the possible options
    let sampler = match otel_config.traces_sampler.as_deref() {
        Some("always_on") => Sampler::AlwaysOn,
        Some("always_off") => Sampler::AlwaysOff,
        Some("traceidratio") => {
            let ratio = otel_config
                .traces_sampler_arg
                .as_ref()
                .and_then(|r| r.parse::<f64>().ok());
            if let Some(r) = ratio {
                Sampler::TraceIdRatioBased(r)
            } else {
                eprintln!("Missing or invalid OTEL_TRACES_SAMPLER_ARG value. Falling back to default: 1.0");
                Sampler::TraceIdRatioBased(1.0)
            }
        }
        Some("parentbased_always_on") => Sampler::ParentBased(Box::new(Sampler::AlwaysOn)),
        Some("parentbased_always_off") => Sampler::ParentBased(Box::new(Sampler::AlwaysOff)),
        Some("parentbased_traceidratio") => {
            let ratio = otel_config
                .traces_sampler_arg
                .as_ref()
                .and_then(|r| r.parse::<f64>().ok());
            if let Some(r) = ratio {
                Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(r)))
            } else {
                eprintln!("Missing or invalid OTEL_TRACES_SAMPLER_ARG value. Falling back to default: 1.0");
                Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(1.0)))
            }
        }
        Some(s) => {
            eprintln!("Unrecognised or unimplemented OTEL_TRACES_SAMPLER value: {s}. Falling back to default: parentbased_always_on");
            Sampler::ParentBased(Box::new(Sampler::AlwaysOn))
        }
        None => Sampler::ParentBased(Box::new(Sampler::AlwaysOn)),
    };

    let mut batch_builder = BatchConfigBuilder::default();
    if let Some(max_batch_queue_size) = otel_config.max_batch_queue_size {
        batch_builder = batch_builder.with_max_queue_size(max_batch_queue_size);
    }
    if let Some(concurrent_exports) = otel_config.concurrent_exports {
        batch_builder = batch_builder.with_max_concurrent_exports(concurrent_exports);
    }
    let batch_config = batch_builder.build();

    let processor =
        opentelemetry_sdk::trace::span_processor_with_async_runtime::BatchSpanProcessor::builder(
            exporter,
            opentelemetry_sdk::runtime::Tokio,
        )
        .with_batch_config(batch_config)
        .build();

    let tracer = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_sampler(sampler)
        .with_resource(
            opentelemetry_sdk::Resource::builder_empty()
                .with_attribute(opentelemetry::KeyValue::new(
                    "service.name",
                    service_name.to_string(),
                ))
                .build(),
        )
        .with_span_processor(processor)
        .build()
        .tracer("wasmcloud-tracing");

    Ok(OpenTelemetryLayer::new(tracer).with_filter(trace_level_filter))
}

#[cfg(feature = "otel")]
fn get_otel_logging_layer<S>(
    service_name: Arc<str>,
    otel_config: &OtelConfig,
    log_level_override: Option<&Level>,
) -> anyhow::Result<impl tracing_subscriber::Layer<S>>
where
    S: Subscriber,
    S: for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use opentelemetry_otlp::WithHttpConfig;

    let exporter = match otel_config.protocol {
        OtelProtocol::Http => {
            let client = crate::get_http_client(otel_config)
                .context("failed to get an http client for otel logging exporter")?;
            opentelemetry_otlp::LogExporter::builder()
                .with_http()
                .with_http_client(client)
                .with_endpoint(otel_config.logs_endpoint())
                .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
                .build()
                .context("failed to create OTEL http log exporter")?
        }
        OtelProtocol::Grpc => {
            // TODO(joonas): Configure tonic::transport::ClientTlsConfig via .with_tls_config(...), passing in additional certificates.
            opentelemetry_otlp::LogExporter::builder()
                .with_tonic()
                .with_endpoint(otel_config.logs_endpoint())
                .build()
                .context("failed to create OTEL http log exporter")?
        }
    };

    let processor =
        opentelemetry_sdk::logs::log_processor_with_async_runtime::BatchLogProcessor::builder(
            exporter,
            opentelemetry_sdk::runtime::Tokio,
        )
        .build();

    let log_provider = opentelemetry_sdk::logs::SdkLoggerProvider::builder()
        .with_resource(
            opentelemetry_sdk::Resource::builder_empty()
                .with_attribute(opentelemetry::KeyValue::new(
                    "service.name",
                    service_name.to_string(),
                ))
                .build(),
        )
        .with_log_processor(processor)
        .build();

    // Prevent the exporter/provider from being dropped
    LOG_PROVIDER
        .set(log_provider)
        .map_err(|_| anyhow::anyhow!("Logger provider already initialized"))?;

    let log_layer = opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
        LOG_PROVIDER.get().unwrap(),
    )
    .with_filter(get_log_level_filter(log_level_override));

    Ok(log_layer)
}

#[cfg(feature = "otel")]
fn get_trace_level_filter(trace_level_override: Option<&Level>) -> EnvFilter {
    if let Some(trace_level) = trace_level_override {
        let level = wasi_level_to_tracing_level(trace_level);
        EnvFilter::default().add_directive(level.into())
    } else {
        EnvFilter::default().add_directive(LevelFilter::DEBUG.into())
    }
}

fn get_log_level_filter(log_level_override: Option<&Level>) -> EnvFilter {
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
            .add_directive("oci_client=info".parse().unwrap());

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
