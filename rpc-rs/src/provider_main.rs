#![cfg(not(target_arch = "wasm32"))]

use crate::async_nats::{AuthError, ConnectOptions};
use std::io::{BufRead, StderrLock, Write};
use std::str::FromStr;

use once_cell::sync::OnceCell;
#[cfg(feature = "otel")]
use opentelemetry::sdk::{
    trace::{self, IdGenerator, Sampler},
    Resource,
};
#[cfg(feature = "otel")]
use opentelemetry_otlp::{Protocol, WithExportConfig};
use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::{
    format::{Format, Full, Json, Writer},
    time::SystemTime,
    FmtContext, FormatEvent, FormatFields,
};
use tracing_subscriber::{
    layer::{Layered, SubscriberExt},
    registry::LookupSpan,
    EnvFilter, Layer, Registry,
};

use crate::{
    core::HostData,
    error::RpcError,
    provider::{HostBridge, ProviderDispatch},
};

lazy_static::lazy_static! {
    static ref STDERR: std::io::Stderr = std::io::stderr();
}

static HOST_DATA: OnceCell<HostData> = OnceCell::new();

struct LockedWriter<'a> {
    stderr: StderrLock<'a>,
}

impl<'a> LockedWriter<'a> {
    fn new() -> Self {
        LockedWriter { stderr: STDERR.lock() }
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

/// singleton host bridge for communicating with the host.
static BRIDGE: OnceCell<HostBridge> = OnceCell::new();

// this may be called any time after initialization
pub fn get_host_bridge() -> &'static HostBridge {
    BRIDGE.get().unwrap_or_else(|| {
        // initialized first thing, so this shouldn't happen
        eprintln!("BRIDGE not initialized");
        panic!();
    })
}

// like get_host_bridge but doesn't panic if it's not initialized
// This could be a valid condition if RpcClient is used outside capability providers
pub fn get_host_bridge_safe() -> Option<&'static HostBridge> {
    BRIDGE.get()
}

#[doc(hidden)]
/// Sets the bridge, return Err if it was already set
pub(crate) fn set_host_bridge(hb: HostBridge) -> Result<(), ()> {
    BRIDGE.set(hb).map_err(|_| ())
}

/// Start provider services: tokio runtime, logger, nats, and rpc subscriptions
pub fn provider_main<P>(
    provider_dispatch: P,
    friendly_name: Option<String>,
) -> Result<(), Box<dyn std::error::Error>>
where
    P: ProviderDispatch + Send + Sync + Clone + 'static,
{
    // get lattice configuration from host
    let host_data = load_host_data().map_err(|e| {
        eprintln!("error loading host data: {}", &e.to_string());
        Box::new(e)
    })?;
    provider_start(provider_dispatch, host_data, friendly_name)
}

/// Start provider services: tokio runtime, logger, nats, and rpc subscriptions,
pub fn provider_start<P>(
    provider_dispatch: P,
    host_data: HostData,
    friendly_name: Option<String>,
) -> Result<(), Box<dyn std::error::Error>>
where
    P: ProviderDispatch + Send + Sync + Clone + 'static,
{
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;

    runtime.block_on(async { provider_run(provider_dispatch, host_data, friendly_name).await })?;
    // in the unlikely case there are any stuck threads,
    // close them so the process has a clean exit
    runtime.shutdown_timeout(core::time::Duration::from_secs(10));
    Ok(())
}

/// Async provider initialization
pub async fn provider_run<P>(
    provider_dispatch: P,
    host_data: HostData,
    friendly_name: Option<String>,
) -> Result<(), Box<dyn std::error::Error>>
where
    P: ProviderDispatch + Send + Sync + Clone + 'static,
{
    configure_tracing(
        friendly_name.unwrap_or_else(|| host_data.provider_key.clone()),
        host_data.structured_logging,
    );

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<bool>(1);

    eprintln!(
        "Starting capability provider {} instance {} with nats url {}",
        &host_data.provider_key, &host_data.instance_id, &host_data.lattice_rpc_url,
    );

    let nats_addr = if !host_data.lattice_rpc_url.is_empty() {
        host_data.lattice_rpc_url.as_str()
    } else {
        crate::provider::DEFAULT_NATS_ADDR
    };
    let nats_server = async_nats::ServerAddr::from_str(nats_addr).map_err(|e| {
        RpcError::InvalidParameter(format!("Invalid nats server url '{nats_addr}': {e}"))
    })?;

    let nc = crate::rpc_client::with_connection_event_logging(
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
    let bridge = HostBridge::new_client(nc, &host_data)?;
    set_host_bridge(bridge).ok();
    let bridge = get_host_bridge();

    // pre-populate provider and bridge with initial set of link definitions
    // initialization of any link is fatal for provider startup
    let initial_links = host_data.link_definitions.clone();
    for ld in initial_links.into_iter() {
        if let Err(e) = provider_dispatch.put_link(&ld).await {
            eprintln!(
                "Failed to initialize link during provider startup - ({:?}): {:?}",
                &ld, e
            );
        } else {
            bridge.put_link(ld).await;
        }
    }

    // subscribe to nats topics
    let _join = bridge
        .connect(
            provider_dispatch,
            &shutdown_tx,
            &host_data.lattice_rpc_prefix,
        )
        .await;

    // run until we receive a shutdown request from host
    let _ = shutdown_rx.recv().await;

    // close chunkifiers
    let _ = tokio::task::spawn_blocking(crate::chunkify::shutdown).await;

    // flush async_nats client
    bridge.flush().await;

    Ok(())
}

/// Loads configuration data sent from the host over stdin. The returned host data contains all the
/// configuration information needed to connect to the lattice and any additional configuration
/// provided to this provider (like `config_json`).
///
/// NOTE: this function will read the data from stdin exactly once. If this function is called more
/// than once, it will return a copy of the original data fetched
pub fn load_host_data() -> Result<HostData, RpcError> {
    // TODO(thomastaylor312): Next time we release a non-patch release, we should have this return a
    // borrowed copy of host data instead rather than cloning every time
    HOST_DATA.get_or_try_init(_load_host_data).cloned()
}

// Internal function for populating the host data
pub fn _load_host_data() -> Result<HostData, RpcError> {
    let mut buffer = String::new();
    let stdin = std::io::stdin();
    {
        let mut handle = stdin.lock();
        handle.read_line(&mut buffer).map_err(|e| {
            RpcError::Rpc(format!(
                "failed to read host data configuration from stdin: {e}"
            ))
        })?;
    }
    // remove spaces, tabs, and newlines before and after base64-encoded data
    let buffer = buffer.trim();
    if buffer.is_empty() {
        return Err(RpcError::Rpc(
            "stdin is empty - expecting host data configuration".to_string(),
        ));
    }
    let bytes = base64::decode(buffer.as_bytes()).map_err(|e| {
        RpcError::Rpc(format!(
            "host data configuration passed through stdin has invalid encoding (expected base64): \
             {e}"
        ))
    })?;
    let host_data: HostData = serde_json::from_slice(&bytes).map_err(|e| {
        RpcError::Rpc(format!(
            "parsing host data: {}:\n{}",
            e,
            String::from_utf8_lossy(&bytes)
        ))
    })?;
    Ok(host_data)
}

#[cfg(feature = "otel")]
const TRACING_PATH: &str = "/v1/traces";

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
fn configure_tracing(_: String, structured_logging_enabled: bool) {
    let filter = get_env_filter();
    let layer = get_log_layer(structured_logging_enabled);
    let subscriber = tracing_subscriber::Registry::default().with(filter).with(layer);
    if let Err(e) = tracing::subscriber::set_global_default(subscriber) {
        eprintln!("Logger was already created by provider, continuing: {e}");
    }
}

#[cfg(feature = "otel")]
fn configure_tracing(provider_name: String, structured_logging_enabled: bool) {
    let env_filter_layer = get_env_filter();
    let log_layer = get_log_layer(structured_logging_enabled);
    let subscriber = tracing_subscriber::Registry::default()
        .with(env_filter_layer)
        .with(log_layer);
    let res = if std::env::var_os("OTEL_TRACES_EXPORTER")
        .unwrap_or_default()
        .to_ascii_lowercase()
        == "otlp"
    {
        let mut tracing_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap_or_else(|_| format!("http://localhost:55681{}", TRACING_PATH));
        if !tracing_endpoint.ends_with(TRACING_PATH) {
            tracing_endpoint.push_str(TRACING_PATH);
        }
        match opentelemetry_otlp::new_pipeline()
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
                    .with_id_generator(IdGenerator::default())
                    .with_max_events_per_span(64)
                    .with_max_attributes_per_span(16)
                    .with_max_events_per_span(16)
                    .with_resource(Resource::new(vec![opentelemetry::KeyValue::new(
                        "service.name",
                        provider_name,
                    )])),
            )
            .install_batch(opentelemetry::runtime::Tokio)
        {
            Ok(t) => tracing::subscriber::set_global_default(
                subscriber.with(tracing_opentelemetry::layer().with_tracer(t)),
            ),
            Err(e) => {
                eprintln!(
                    "Unable to configure OTEL tracing, defaulting to logging only: {:?}",
                    e
                );
                tracing::subscriber::set_global_default(subscriber)
            }
        }
    } else {
        tracing::subscriber::set_global_default(subscriber)
    };
    if let Err(e) = res {
        eprintln!(
            "Logger/tracer was already created by provider, continuing: {}",
            e
        );
    }
}

fn get_log_layer(structured_logging_enabled: bool) -> impl Layer<Layered<EnvFilter, Registry>> {
    let log_layer = tracing_subscriber::fmt::layer()
        .with_writer(LockedWriter::new)
        .with_ansi(atty::is(atty::Stream::Stderr));
    if structured_logging_enabled {
        log_layer.event_format(JsonOrNot::Json(Format::default().json()))
    } else {
        log_layer.event_format(JsonOrNot::Not(Format::default()))
    }
}

fn get_env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|e| {
        eprintln!("RUST_LOG was not set or the given directive was invalid: {e:?}\nDefaulting logger to `info` level");
        EnvFilter::default().add_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
    })
}
