//! Functions for starting and running a provider

use std::io::{BufRead, IsTerminal, StderrLock, Write};
use std::str::FromStr;

use async_nats::{AuthError, ConnectOptions};
use base64::Engine;
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
use tracing::{error, info, Event, Subscriber};
use tracing_subscriber::fmt::format::{DefaultFields, JsonFields};
use tracing_subscriber::fmt::{
    format::{Format, Full, Json, Writer},
    time::SystemTime,
    FmtContext, FormatEvent, FormatFields,
};
use tracing_subscriber::{
    filter::LevelFilter,
    layer::{Layered, SubscriberExt},
    registry::LookupSpan,
    EnvFilter, Layer, Registry,
};

use crate::error::{ProviderError, ProviderResult};
use crate::provider::ProviderConnection;
use crate::{
    core::{logging::Level, HostData, OtelConfig},
    Provider,
};

lazy_static::lazy_static! {
    static ref STDERR: std::io::Stderr = std::io::stderr();
}

static HOST_DATA: OnceCell<HostData> = OnceCell::new();
static CONNECTION: OnceCell<ProviderConnection> = OnceCell::new();

struct LockedWriter<'a> {
    stderr: StderrLock<'a>,
}

impl<'a> LockedWriter<'a> {
    fn new() -> Self {
        LockedWriter {
            stderr: STDERR.lock(),
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

/// Retrieves the currently configured connection to the lattice. DO NOT call this method until
/// after the provider is running (meaning [`start_provider`] or [`run_provider`] have been called)
/// or this method will panic. Only in extremely rare cases should this be called manually and it
/// will only be used by generated code
// NOTE(thomastaylor312): This isn't the most elegant solution, but providers that need to send
// messages to the lattice rather than just responding need to get the same connection used when the
// provider was started, which means a global static
pub fn get_connection() -> &'static ProviderConnection {
    CONNECTION
        .get()
        .expect("Provider connection not initialized")
}

/// Starts a provider, reading all of the host data and starting the process
pub fn start_provider<P>(provider: P, friendly_name: Option<String>) -> ProviderResult<()>
where
    P: Provider + Clone,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| ProviderError::Initialization(e.to_string()))?;

    runtime.block_on(run_provider(provider, friendly_name))?;
    // in the unlikely case there are any stuck threads,
    // close them so the process has a clean exit
    runtime.shutdown_timeout(std::time::Duration::from_secs(10));
    Ok(())
}

/// Runs the provider. You can use this method instead of [`start_provider`] if you are already in
/// an async context
pub async fn run_provider<P>(provider: P, friendly_name: Option<String>) -> ProviderResult<()>
where
    P: Provider + Clone,
{
    let host_data = tokio::task::spawn_blocking(load_host_data)
        .await
        .map_err(|e| ProviderError::Initialization(format!("Unable to load host data: {e}")))??;
    configure_tracing(
        friendly_name.unwrap_or(host_data.provider_key.clone()),
        &host_data.otel_config,
        host_data.structured_logging,
        &host_data.log_level,
    );

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<bool>(1);

    info!(
        "Starting capability provider {} instance {} with nats url {}",
        &host_data.provider_key, &host_data.instance_id, &host_data.lattice_rpc_url,
    );

    let nats_addr = if !host_data.lattice_rpc_url.is_empty() {
        host_data.lattice_rpc_url.as_str()
    } else {
        crate::DEFAULT_NATS_ADDR
    };
    let nats_server = async_nats::ServerAddr::from_str(nats_addr).map_err(|e| {
        ProviderError::Initialization(format!("Invalid nats server url '{nats_addr}': {e}"))
    })?;

    let nc = crate::with_connection_event_logging(
        match (
            host_data.lattice_rpc_user_jwt.trim(),
            host_data.lattice_rpc_user_seed.trim(),
        ) {
            ("", "") => ConnectOptions::default(),
            (rpc_jwt, rpc_seed) => {
                let key_pair = std::sync::Arc::new(nkeys::KeyPair::from_seed(rpc_seed).unwrap());
                let jwt = rpc_jwt.to_owned();
                ConnectOptions::with_jwt(jwt, move |nonce| {
                    let key_pair = key_pair.clone();
                    async move { key_pair.sign(&nonce).map_err(AuthError::new) }
                })
            }
        },
    )
    .connect(nats_server)
    .await?;

    // initialize HostBridge
    let connection = ProviderConnection::new(nc, host_data)?;
    CONNECTION.set(connection).map_err(|_| {
        ProviderError::Initialization("Provider connection was already initialized".to_string())
    })?;
    let connection = get_connection();

    // pre-populate provider and bridge with initial set of link definitions
    // initialization of any link is fatal for provider startup
    let initial_links = host_data.link_definitions.clone();
    for ld in initial_links.into_iter() {
        if !provider.put_link(&ld).await {
            error!(
                link_definition = ?ld,
                "Failed to initialize link during provider startup",
            );
        } else {
            connection.put_link(ld).await;
        }
    }

    // subscribe to nats topics
    connection
        .connect(provider, &shutdown_tx, &host_data.lattice_rpc_prefix)
        .await?;

    // run until we receive a shutdown request from host
    let _ = shutdown_rx.recv().await;

    // flush async_nats client
    connection.flush().await;

    Ok(())
}

/// Loads configuration data sent from the host over stdin. The returned host data contains all the
/// configuration information needed to connect to the lattice and any additional configuration
/// provided to this provider (like `config_json`).
///
/// NOTE: this function will read the data from stdin exactly once. If this function is called more
/// than once, it will return a copy of the original data fetched
pub fn load_host_data() -> ProviderResult<&'static HostData> {
    HOST_DATA.get_or_try_init(_load_host_data)
}

// Internal function for populating the host data
fn _load_host_data() -> ProviderResult<HostData> {
    let mut buffer = String::new();
    let stdin = std::io::stdin();
    {
        let mut handle = stdin.lock();
        handle.read_line(&mut buffer).map_err(|e| {
            ProviderError::Initialization(format!(
                "failed to read host data configuration from stdin: {e}"
            ))
        })?;
    }
    // remove spaces, tabs, and newlines before and after base64-encoded data
    let buffer = buffer.trim();
    if buffer.is_empty() {
        return Err(ProviderError::Initialization(
            "stdin is empty - expecting host data configuration".to_string(),
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(buffer.as_bytes())
        .map_err(|e| {
            ProviderError::Initialization(format!(
            "host data configuration passed through stdin has invalid encoding (expected base64): \
             {e}"
        ))
        })?;
    let host_data: HostData = serde_json::from_slice(&bytes).map_err(|e| {
        ProviderError::Initialization(format!(
            "parsing host data: {}:\n{}",
            e,
            String::from_utf8_lossy(&bytes)
        ))
    })?;
    Ok(host_data)
}

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
fn configure_tracing(
    _: String,
    _: &Option<OtelConfig>,
    structured_logging_enabled: bool,
    log_level_override: &Option<Level>,
) {
    let base_reg = tracing_subscriber::Registry::default();
    let level_filter = get_level_filter(log_level_override);

    let res = if structured_logging_enabled {
        let log_layer = get_json_log_layer();
        let layered = base_reg.with(level_filter).with(log_layer);
        tracing::subscriber::set_global_default(layered)
    } else {
        let log_layer = get_default_log_layer();
        let layered = base_reg.with(level_filter).with(log_layer);
        tracing::subscriber::set_global_default(layered)
    };

    if let Err(err) = res {
        eprintln!(
            "Logger/tracer was already created by provider, continuing: {}",
            err
        );
    }
}

#[cfg(feature = "otel")]
fn configure_tracing(
    provider_name: String,
    otel_config: &Option<OtelConfig>,
    structured_logging_enabled: bool,
    log_level_override: &Option<Level>,
) {
    let base_reg = tracing_subscriber::Registry::default();
    let level_filter = get_level_filter(log_level_override);

    let exporter = otel_config
        .as_ref()
        .map(|c| c.traces_exporter.as_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let maybe_tracer = if exporter.is_empty() {
        None
    } else if exporter != "otlp" {
        eprintln!("unsupported OTEL exporter: '{}'", exporter);
        None
    } else {
        let mut tracing_endpoint = otel_config
            .as_ref()
            .map(|c| c.exporter_otlp_endpoint.clone())
            .unwrap_or_default();

        if tracing_endpoint.is_empty() {
            eprintln!("OTEL exporter endpoint not set, defaulting to '{DEFAULT_TRACING_ENDPOINT}'");
            tracing_endpoint = DEFAULT_TRACING_ENDPOINT.to_string();
        }

        Some(get_tracer(tracing_endpoint, provider_name))
    };

    let res = match (maybe_tracer, structured_logging_enabled) {
        (Some(Ok(t)), true) => {
            let log_layer = get_json_log_layer();
            let tracing_layer = tracing_opentelemetry::layer().with_tracer(t);
            let layered = base_reg
                .with(level_filter)
                .with(log_layer)
                .with(tracing_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (Some(Ok(t)), false) => {
            let log_layer = get_default_log_layer();
            let tracing_layer = tracing_opentelemetry::layer().with_tracer(t);
            let layered = base_reg
                .with(level_filter)
                .with(log_layer)
                .with(tracing_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (Some(Err(err)), true) => {
            eprintln!("Unable to configure OTEL tracing, defaulting to logging only: {err:?}");
            let log_layer = get_json_log_layer();
            let layered = base_reg.with(level_filter).with(log_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (Some(Err(err)), false) => {
            eprintln!("Unable to configure OTEL tracing, defaulting to logging only: {err:?}");
            let log_layer = get_default_log_layer();
            let layered = base_reg.with(level_filter).with(log_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (None, true) => {
            let log_layer = get_json_log_layer();
            let layered = base_reg.with(level_filter).with(log_layer);
            tracing::subscriber::set_global_default(layered)
        }
        (None, false) => {
            let log_layer = get_default_log_layer();
            let layered = base_reg.with(level_filter).with(log_layer);
            tracing::subscriber::set_global_default(layered)
        }
    };

    if let Err(err) = res {
        eprintln!(
            "Logger/tracer was already created by provider, continuing: {}",
            err
        );
    }
}

#[cfg(feature = "otel")]
fn get_tracer(mut tracing_endpoint: String, provider_name: String) -> Result<Tracer, TraceError> {
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
                    provider_name,
                )])),
        )
        .install_batch(opentelemetry::runtime::Tokio)
}

fn get_default_log_layer() -> impl Layer<Layered<EnvFilter, Registry>> {
    tracing_subscriber::fmt::layer()
        .with_writer(LockedWriter::new)
        .with_ansi(STDERR.is_terminal())
        .event_format(JsonOrNot::Not(Format::default()))
        .fmt_fields(DefaultFields::new())
}

fn get_json_log_layer() -> impl Layer<Layered<EnvFilter, Registry>> {
    tracing_subscriber::fmt::layer()
        .with_writer(LockedWriter::new)
        .with_ansi(STDERR.is_terminal())
        .event_format(JsonOrNot::Json(Format::default().json()))
        .fmt_fields(JsonFields::new())
}

fn get_level_filter(log_level_override: &Option<Level>) -> EnvFilter {
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
