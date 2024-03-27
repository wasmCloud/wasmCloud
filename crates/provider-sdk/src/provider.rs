use core::fmt;
use core::fmt::Formatter;
use core::future::Future;

use core::time::Duration;
use std::collections::HashMap;
use std::io::BufRead;
use std::sync::Arc;

use anyhow::{bail, Result};
use async_nats::subject::ToSubject;
use async_nats::HeaderMap;
use base64::Engine;
use futures::{StreamExt, TryStreamExt};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, RwLock};
use tokio::task::{spawn_blocking, JoinHandle};
use tokio::{select, spawn, try_join};
use tracing::{debug, error, info, instrument, trace, warn, Instrument as _};
use ulid::Ulid;
use uuid::Uuid;
use wasmcloud_core::nats::convert_header_map_to_hashmap;
use wasmcloud_core::rpc::{health_subject, link_del_subject, link_put_subject, shutdown_subject};
use wasmcloud_core::{HealthCheckRequest, HealthCheckResponse, HostData, InterfaceLinkDefinition};
use wrpc_transport::{AcceptedInvocation, Client, Transmitter};
use wrpc_types::DynamicFunction;

#[cfg(feature = "otel")]
use wasmcloud_core::TraceContext;
#[cfg(feature = "otel")]
use wasmcloud_tracing::context::attach_span_context;

use crate::error::{InvocationResult, ProviderInitError, ProviderInitResult};
use crate::{
    with_connection_event_logging, Context, Provider, ProviderHandler, WrpcDispatch,
    WrpcInvocationLookup, DEFAULT_NATS_ADDR,
};

/// Name of the header that should be passed for invocations that identifies the source
const WRPC_SOURCE_ID_HEADER_NAME: &str = "source-id";

/// Name of the header that should be passed for invocations that identifies the host from which invocation was run
const WRPC_HEADER_NAME_HOST_ID: &str = "host-id";

static HOST_DATA: OnceCell<HostData> = OnceCell::new();
static CONNECTION: OnceCell<ProviderConnection> = OnceCell::new();

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

/// Loads configuration data sent from the host over stdin. The returned host data contains all the
/// configuration information needed to connect to the lattice and any additional configuration
/// provided to this provider (like `config_json`).
///
/// NOTE: this function will read the data from stdin exactly once. If this function is called more
/// than once, it will return a copy of the original data fetched
pub fn load_host_data() -> ProviderInitResult<&'static HostData> {
    HOST_DATA.get_or_try_init(_load_host_data)
}

// Internal function for populating the host data
fn _load_host_data() -> ProviderInitResult<HostData> {
    let mut buffer = String::new();
    let stdin = std::io::stdin();
    {
        let mut handle = stdin.lock();
        handle.read_line(&mut buffer).map_err(|e| {
            ProviderInitError::Initialization(format!(
                "failed to read host data configuration from stdin: {e}"
            ))
        })?;
    }
    // remove spaces, tabs, and newlines before and after base64-encoded data
    let buffer = buffer.trim();
    if buffer.is_empty() {
        return Err(ProviderInitError::Initialization(
            "stdin is empty - expecting host data configuration".to_string(),
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(buffer.as_bytes())
        .map_err(|e| {
            ProviderInitError::Initialization(format!(
            "host data configuration passed through stdin has invalid encoding (expected base64): \
             {e}"
        ))
        })?;
    let host_data: HostData = serde_json::from_slice(&bytes).map_err(|e| {
        ProviderInitError::Initialization(format!(
            "parsing host data: {}:\n{}",
            e,
            String::from_utf8_lossy(&bytes)
        ))
    })?;
    Ok(host_data)
}

/// Current version of wRPC supported by this version of the provider-sdk
pub(crate) const WRPC_VERSION: &str = "0.0.1";

pub type QuitSignal = broadcast::Receiver<()>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ShutdownMessage {
    /// The ID of the host that sent the message
    pub host_id: String,
}

#[doc(hidden)]
/// Process subscription, until closed or exhausted, or value is received on the channel.
/// `sub` is a mutable Subscriber (regular or queue subscription)
/// `channel` may be either tokio mpsc::Receiver or broadcast::Receiver, and is considered signaled
/// when a value is sent or the chanel is closed.
/// `msg` is the variable name to be used in the handler
/// `on_item` is an async handler
macro_rules! process_until_quit {
    ($sub:ident, $channel:ident, $msg:ident, $on_item:tt) => {
        spawn(async move {
            loop {
                select! {
                    _ = $channel.recv() => {
                        let _ = $sub.unsubscribe().await;
                        break;
                    },
                    __msg = $sub.next() => {
                        match __msg {
                            None => break,
                            Some($msg) => $on_item
                        }
                    }
                }
            }
        })
    };
}

async fn subscribe_health(
    nats: Arc<async_nats::Client>,
    mut quit: broadcast::Receiver<()>,
    lattice: &str,
    provider_key: &str,
) -> ProviderInitResult<mpsc::Receiver<(HealthCheckRequest, oneshot::Sender<HealthCheckResponse>)>>
{
    let mut sub = nats
        .subscribe(health_subject(lattice, provider_key))
        .await?;
    let (health_tx, health_rx) = mpsc::channel(1);
    spawn({
        let nats = Arc::clone(&nats);
        async move {
            process_until_quit!(sub, quit, msg, {
                let (tx, rx) = oneshot::channel();
                if let Err(err) = health_tx.send((HealthCheckRequest {}, tx)).await {
                    error!(%err, "failed to send health check request");
                    continue;
                }
                match rx.await.as_ref().map(serde_json::to_vec) {
                    Err(err) => {
                        error!(%err, "failed to receive health check response")
                    }
                    Ok(Ok(t)) => {
                        if let Some(reply_to) = msg.reply {
                            if let Err(err) = nats.publish(reply_to, t.into()).await {
                                error!(%err, "failed sending health check response");
                            }
                        }
                    }
                    Ok(Err(err)) => {
                        // extremely unlikely that InvocationResponse would fail to serialize
                        error!(%err, "failed serializing HealthCheckResponse");
                    }
                }
            });
        }
        .instrument(tracing::debug_span!("subscribe_health"))
    });
    Ok(health_rx)
}

async fn subscribe_shutdown(
    nats: Arc<async_nats::Client>,
    quit: broadcast::Sender<()>,
    lattice: &str,
    provider_key: &str,
    link_name: &str,
    host_id: &'static str,
) -> ProviderInitResult<mpsc::Receiver<oneshot::Sender<()>>> {
    let mut sub = nats
        .subscribe(shutdown_subject(lattice, provider_key, link_name))
        .await?;
    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    spawn({
        async move {
            loop {
                let msg = sub.next().await;
                // Check if we really need to shut down
                if let Some(async_nats::Message {
                    reply: Some(reply_to),
                    payload,
                    ..
                }) = msg
                {
                    let ShutdownMessage {
                        host_id: ref req_host_id,
                    } = serde_json::from_slice(&payload).unwrap_or_default();
                    if req_host_id == host_id {
                        info!("Received termination signal and stopping");
                        // Tell provider to shutdown - before we shut down nats subscriptions,
                        // in case it needs to do any message passing during shutdown
                        let (tx, rx) = oneshot::channel();
                        match shutdown_tx.send(tx).await {
                            Ok(()) => {
                                if let Err(err) = rx.await {
                                    error!(%err, "failed to await shutdown")
                                }
                            }
                            Err(err) => error!(%err, "failed to send shutdown"),
                        }
                        if let Err(err) = nats.publish(reply_to, "shutting down".into()).await {
                            warn!(%err, "failed to send shutdown ack");
                        }
                        // unsubscribe from shutdown topic
                        if let Err(err) = sub.unsubscribe().await {
                            warn!(%err, "failed to unsubscribe from shutdown topic")
                        }
                        // send shutdown signal to all listeners: quit all subscribers and signal main thread to quit
                        if let Err(err) = quit.send(()) {
                            error!(%err, "Problem shutting down:  failure to send signal");
                        }
                        break;
                    }
                    trace!("Ignoring termination signal (request targeted for different host)");
                }
            }
        }
        .instrument(tracing::debug_span!("shutdown_subscriber"))
    });
    Ok(shutdown_rx)
}

async fn subscribe_link_put(
    nats: Arc<async_nats::Client>,
    mut quit: broadcast::Receiver<()>,
    lattice: &str,
    provider_key: &str,
) -> ProviderInitResult<mpsc::Receiver<(InterfaceLinkDefinition, oneshot::Sender<()>)>> {
    let mut sub = nats
        .subscribe(link_put_subject(lattice, provider_key))
        .await?;
    let (link_put_tx, link_put_rx) = mpsc::channel(1);
    spawn(async move {
        process_until_quit!(sub, quit, msg, {
            match serde_json::from_slice::<InterfaceLinkDefinition>(&msg.payload) {
                Ok(ld) => {
                    let span = tracing::Span::current();
                    span.record("source_id", &tracing::field::display(&ld.source_id));
                    span.record("target", &tracing::field::display(&ld.target));
                    span.record("wit_namespace", &tracing::field::display(&ld.wit_namespace));
                    span.record("wit_package", &tracing::field::display(&ld.wit_package));
                    span.record(
                        "wit_interfaces",
                        &tracing::field::display(&ld.interfaces.join(",")),
                    );
                    span.record("link_name", &tracing::field::display(&ld.name));
                    let (tx, rx) = oneshot::channel();
                    if let Err(err) = link_put_tx.send((ld, tx)).await {
                        error!(%err, "failed to send link put request");
                        continue;
                    }
                    if let Err(err) = rx.await {
                        error!(%err, "failed to await link_put")
                    }
                }
                Err(err) => {
                    error!(%err, "received invalid link def data on message");
                }
            }
        });
    });
    Ok(link_put_rx)
}

async fn subscribe_link_del(
    nats: Arc<async_nats::Client>,
    mut quit: broadcast::Receiver<()>,
    lattice: &str,
    provider_key: &str,
) -> ProviderInitResult<mpsc::Receiver<(InterfaceLinkDefinition, oneshot::Sender<()>)>> {
    let subject = link_del_subject(lattice, provider_key).to_subject();
    debug!(%subject, "subscribing for link del");
    let mut sub = nats.subscribe(subject.clone()).await?;
    let (link_del_tx, link_del_rx) = mpsc::channel(1);
    let span = tracing::trace_span!("subscribe_link_del", %subject);
    spawn(
        async move {
            process_until_quit!(sub, quit, msg, {
                if let Ok(ld) = serde_json::from_slice::<InterfaceLinkDefinition>(&msg.payload) {
                    let (tx, rx) = oneshot::channel();
                    if let Err(err) = link_del_tx.send((ld, tx)).await {
                        error!(%err, "failed to send link del request");
                        continue;
                    }
                    if let Err(err) = rx.await {
                        error!(%err, "failed to await link_del")
                    }
                }
            });
        }
        .instrument(span),
    );
    Ok(link_del_rx)
}

pub(crate) struct ProviderCommandReceivers {
    pub health: mpsc::Receiver<(HealthCheckRequest, oneshot::Sender<HealthCheckResponse>)>,
    pub shutdown: mpsc::Receiver<oneshot::Sender<()>>,
    pub link_put: mpsc::Receiver<(InterfaceLinkDefinition, oneshot::Sender<()>)>,
    pub link_del: mpsc::Receiver<(InterfaceLinkDefinition, oneshot::Sender<()>)>,
}

/// State of provider initialization
pub(crate) struct ProviderInitState {
    pub nats: Arc<async_nats::Client>,
    pub quit_rx: broadcast::Receiver<()>,
    pub quit_tx: broadcast::Sender<()>,
    pub host_id: String,
    pub lattice_rpc_prefix: String,
    pub link_name: String,
    pub provider_key: String,
    pub link_definitions: Vec<InterfaceLinkDefinition>,
    pub commands: ProviderCommandReceivers,
    pub config: HashMap<String, String>,
}

#[instrument]
async fn init_provider(name: &str) -> ProviderInitResult<ProviderInitState> {
    let HostData {
        host_id,
        lattice_rpc_prefix,
        link_name,
        lattice_rpc_user_jwt,
        lattice_rpc_user_seed,
        lattice_rpc_url,
        provider_key,
        invocation_seed: _,
        env_values: _,
        instance_id,
        link_definitions,
        cluster_issuers: _,
        config,
        default_rpc_timeout_ms: _,
        structured_logging,
        log_level,
        otel_config,
    } = spawn_blocking(load_host_data).await.map_err(|e| {
        ProviderInitError::Initialization(format!("failed to load host data: {e}"))
    })??;

    if let Err(err) = wasmcloud_tracing::configure_observability(
        name,
        otel_config,
        *structured_logging,
        log_level.as_ref(),
    ) {
        error!(?err, "failed to configure tracing");
    }

    let (quit_tx, quit_rx) = broadcast::channel(1);

    info!(
        "Starting capability provider {provider_key} instance {instance_id} with nats url {lattice_rpc_url}"
    );

    let nats_addr = if !lattice_rpc_url.is_empty() {
        lattice_rpc_url.as_str()
    } else {
        DEFAULT_NATS_ADDR
    };
    let nats = with_connection_event_logging(
        match (lattice_rpc_user_jwt.trim(), lattice_rpc_user_seed.trim()) {
            ("", "") => async_nats::ConnectOptions::default(),
            (rpc_jwt, rpc_seed) => {
                let key_pair = Arc::new(nkeys::KeyPair::from_seed(rpc_seed).unwrap());
                let jwt = rpc_jwt.to_owned();
                async_nats::ConnectOptions::with_jwt(jwt, move |nonce| {
                    let key_pair = key_pair.clone();
                    async move { key_pair.sign(&nonce).map_err(async_nats::AuthError::new) }
                })
            }
        },
    )
    .connect(nats_addr)
    .await?;
    let nats = Arc::new(nats);
    let (health, shutdown, link_put, link_del) = try_join!(
        subscribe_health(
            Arc::clone(&nats),
            quit_tx.subscribe(),
            lattice_rpc_prefix,
            provider_key
        ),
        subscribe_shutdown(
            Arc::clone(&nats),
            quit_tx.clone(),
            lattice_rpc_prefix,
            provider_key,
            link_name,
            host_id
        ),
        subscribe_link_put(
            Arc::clone(&nats),
            quit_tx.subscribe(),
            lattice_rpc_prefix,
            provider_key,
        ),
        subscribe_link_del(
            Arc::clone(&nats),
            quit_tx.subscribe(),
            lattice_rpc_prefix,
            provider_key,
        ),
    )?;
    Ok(ProviderInitState {
        nats,
        quit_rx,
        quit_tx,
        host_id: host_id.clone(),
        lattice_rpc_prefix: lattice_rpc_prefix.clone(),
        link_name: link_name.clone(),
        provider_key: provider_key.clone(),
        link_definitions: link_definitions.clone(),
        config: config.clone(),
        commands: ProviderCommandReceivers {
            health,
            shutdown,
            link_put,
            link_del,
        },
    })
}

/// Starts a provider, reading all of the host data and starting the process
pub fn start_provider(
    provider: impl Provider + Clone,
    friendly_name: &str,
) -> ProviderInitResult<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| ProviderInitError::Initialization(e.to_string()))?;

    runtime.block_on(run_provider(provider, friendly_name))?;
    // in the unlikely case there are any stuck threads,
    // close them so the process has a clean exit
    runtime.shutdown_timeout(std::time::Duration::from_secs(10));
    Ok(())
}

/// Appropriately receive a link (depending on if it's source/target) for a provider
async fn receive_link_for_provider<P>(
    provider: &P,
    connection: &ProviderConnection,
    ld: InterfaceLinkDefinition,
) -> Result<()>
where
    P: ProviderHandler,
{
    let do_receive_link = if ld.source_id == connection.provider_key {
        provider.receive_link_config_as_source((
            &ld.source_id,
            &ld.target,
            &ld.name,
            &ld.source_config,
        ))
    } else if ld.target == connection.provider_key {
        provider.receive_link_config_as_target((
            &ld.source_id,
            &ld.target,
            &ld.name,
            &ld.target_config,
        ))
    } else {
        bail!("received link put where provider was neither source nor target");
    };

    match do_receive_link.await {
        Ok(()) => connection.put_link(ld).await,
        Err(e) => {
            warn!(error = %e, "receiving link failed");
        }
    };

    Ok(())
}

/// Handle provider commands in a loop.
async fn handle_provider_commands(
    provider: impl ProviderHandler,
    connection: &ProviderConnection,
    mut quit_rx: broadcast::Receiver<()>,
    quit_tx: broadcast::Sender<()>,
    ProviderCommandReceivers {
        mut health,
        mut shutdown,
        mut link_put,
        mut link_del,
    }: ProviderCommandReceivers,
) {
    loop {
        select! {
            // run until we receive a shutdown request from host
            _ = quit_rx.recv() => {
                // flush async_nats client
                connection.flush().await;
                return
            }
            req = health.recv() => {
                if let Some((req, tx)) = req {
                    let res = match provider.health_request(&req).await {
                        Ok(v) => v,
                        Err(e) => {
                            error!(error = %e, "provider health request failed");
                            return;
                        }
                    };
                    if tx.send(res).is_err() {
                        error!("failed to send health check response")
                    }
                } else {
                    error!("failed to handle health check, shutdown");
                    if let Err(e) = provider.shutdown().await {
                        error!(error = %e, "failed to shutdown provider");
                    }
                    if quit_tx.send(()).is_err() {
                        error!("failed to send quit")
                    };
                    return
                };
            }
            req = shutdown.recv() => {
                if let Some(tx) = req {
                    if let Err(e) = provider.shutdown().await {
                        error!(error = %e, "failed to shutdown provider");
                    }
                    if tx.send(()).is_err() {
                        error!("failed to send shutdown response")
                    }
                } else {
                    error!("failed to handle shutdown, shutdown");
                    if let Err(e) = provider.shutdown().await {
                        error!(error = %e, "failed to shutdown provider");
                    }
                    if quit_tx.send(()).is_err() {
                        error!("failed to send quit")
                    };
                    return
                };
            }
            req = link_put.recv() => {
                if let Some((ld, tx)) = req {
                    // If the link has already been put, return early
                    if connection.is_linked(&ld.source_id).await {
                        warn!("Ignoring duplicate link put");
                    } else {
                        info!("Linking actor with provider");
                        if let Err(e) = receive_link_for_provider(&provider, connection, ld).await {
                            error!(error = %e, "failed to receive link for provider");
                        }
                    }
                    if tx.send(()).is_err() {
                        error!("failed to send link put response")
                    }
                } else {
                    error!("failed to handle link put, shutdown");
                    if let Err(e) = provider.shutdown().await {
                        error!(error = %e, "failed to shutdown provider");
                    }
                    if quit_tx.send(()).is_err() {
                        error!("failed to send quit")
                    };
                    return
                };
            }
            req = link_del.recv() => {
                if let Some((ld, tx)) = req {
                    connection.delete_link(&ld.source_id).await;
                    // notify provider that link is deleted
                    if let Err(e) = provider.delete_link(&ld.source_id).await {
                        error!(error = %e, "failed to delete link");
                    }
                    if tx.send(()).is_err() {
                        error!("failed to send link del response")
                    }
                } else {
                    error!("failed to handle link del, shutdown");
                    if let Err(e) = provider.shutdown().await {
                        error!(error = %e, "failed to shutdown provider");
                    }
                    if quit_tx.send(()).is_err() {
                        error!("failed to send quit")
                    };
                    return
                };
            }
        }
    }
}

/// Runs the provider handler. You can use this method instead of [`start_provider`] if you are already in
/// an async context and want to manually manage RPC serving functionality.
pub async fn run_provider_handler(
    provider: impl ProviderHandler,
    friendly_name: &str,
) -> ProviderInitResult<impl Future<Output = ()>> {
    let init_state = init_provider(friendly_name).await?;

    // Run user-implemented provider-internal specific initialization
    if let Err(e) = provider.init(&init_state).await {
        return Err(ProviderInitError::Initialization(format!(
            "provider init failed: {e}"
        )));
    }

    let ProviderInitState {
        nats,
        quit_rx,
        quit_tx,
        host_id,
        lattice_rpc_prefix,
        link_name,
        provider_key,
        link_definitions,
        commands,
        config,
    } = init_state;

    let connection = ProviderConnection::new(
        Arc::clone(&nats),
        provider_key,
        lattice_rpc_prefix.clone(),
        host_id,
        link_name,
        WrpcInvocationLookup::default(),
        config,
    )?;
    CONNECTION.set(connection).map_err(|_| {
        ProviderInitError::Initialization("Provider connection was already initialized".to_string())
    })?;
    let connection = get_connection();

    // Pre-populate provider and bridge with initial set of link definitions
    // Initialization of any link is fatal for provider startup
    for ld in link_definitions {
        if let Err(e) = receive_link_for_provider(&provider, connection, ld).await {
            error!(
                error = %e,
                "failed to initialize link during provider startup",
            );
        }
    }
    Ok(handle_provider_commands(
        provider, connection, quit_rx, quit_tx, commands,
    ))
}

/// Runs the provider. You can use this method instead of [`start_provider`] if you are already in
/// an async context
pub async fn run_provider(
    provider: impl Provider + Clone,
    friendly_name: &str,
) -> ProviderInitResult<()> {
    let init_state = init_provider(friendly_name).await?;

    // Run user-implemented provider-internal specific initialization
    if let Err(e) = provider.init(&init_state).await {
        return Err(ProviderInitError::Initialization(format!(
            "provider init failed: {e}"
        )));
    }

    let ProviderInitState {
        nats,
        quit_rx,
        quit_tx,
        host_id,
        lattice_rpc_prefix,
        link_name,
        provider_key,
        link_definitions,
        commands,
        config,
    } = init_state;

    let invocation_map = provider
        .incoming_wrpc_invocations_by_subject(
            &lattice_rpc_prefix,
            &provider_key,
            crate::provider::WRPC_VERSION,
        )
        .await?;

    // Initialize host connection to provider, save it as a global
    let connection = ProviderConnection::new(
        nats,
        provider_key,
        lattice_rpc_prefix.clone(),
        host_id,
        link_name,
        invocation_map,
        config,
    )?;
    CONNECTION.set(connection).map_err(|_| {
        ProviderInitError::Initialization("Provider connection was already initialized".to_string())
    })?;
    let connection = get_connection();

    // Pre-populate provider and bridge with initial set of link definitions
    // Initialization of any link is fatal for provider startup
    for ld in link_definitions {
        if let Err(e) = receive_link_for_provider(&provider, connection, ld).await {
            error!(
                error = ?e,
                "failed to initialize link during provider startup",
            );
        }
    }
    connection
        .subscribe_rpc(
            provider.clone(),
            quit_tx.subscribe(),
            lattice_rpc_prefix,
            &connection.provider_key,
        )
        .await?;

    handle_provider_commands(provider, connection, quit_rx, quit_tx, commands).await;
    Ok(())
}

/// Source ID for a link
type SourceId = String;

#[derive(Clone)]
pub struct ProviderConnection {
    /// Links currently active on the provider, by Source ID
    links: Arc<RwLock<HashMap<SourceId, InterfaceLinkDefinition>>>,

    /// NATS client used for performing RPCs
    nats: Arc<async_nats::Client>,

    /// Lattice name
    lattice: String,
    host_id: String,
    link_name: String,
    provider_key: String,

    // TODO: Reference this field to get static config
    #[allow(unused)]
    config: HashMap<String, String>,

    /// Mapping of NATS subjects to dynamic function information for incoming invocations
    #[allow(unused)]
    incoming_invocation_fn_map: Arc<WrpcInvocationLookup>,
}

impl fmt::Debug for ProviderConnection {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConnection")
            .field("provider_id", &self.provider_key())
            .field("host_id", &self.host_id)
            .field("link", &self.link_name)
            .field("lattice", &self.lattice)
            .finish()
    }
}

/// Extracts trace context from incoming headers
pub fn invocation_context(headers: &HeaderMap) -> Context {
    #[cfg(feature = "otel")]
    {
        let trace_context: TraceContext = convert_header_map_to_hashmap(headers)
            .into_iter()
            .collect::<Vec<(String, String)>>();
        attach_span_context(&trace_context);
    }
    // Determine source ID for the invocation
    let source_id = headers
        .get(WRPC_SOURCE_ID_HEADER_NAME)
        .map(ToString::to_string)
        .unwrap_or_else(|| "<unknown>".into());
    Context {
        actor: Some(source_id),
        tracing: convert_header_map_to_hashmap(headers),
    }
}

impl ProviderConnection {
    pub(crate) fn new(
        nats: Arc<async_nats::Client>,
        provider_key: String,
        lattice: String,
        host_id: String,
        link_name: String,
        incoming_invocation_fn_map: WrpcInvocationLookup,
        config: HashMap<String, String>,
    ) -> ProviderInitResult<ProviderConnection> {
        Ok(ProviderConnection {
            links: Arc::default(),
            nats,
            lattice,
            host_id,
            link_name,
            provider_key,
            incoming_invocation_fn_map: Arc::new(incoming_invocation_fn_map),
            config,
        })
    }

    /// Used for fetching the RPC client in order to make RPC calls
    pub fn get_wrpc_client(&self, target: &str) -> wasmcloud_core::wrpc::Client {
        let mut headers = HeaderMap::new();
        headers.insert("source-id", self.provider_key.as_str());
        headers.insert("target-id", target);
        wasmcloud_core::wrpc::Client::new(
            Arc::clone(&self.nats),
            &self.lattice,
            target,
            headers,
            Duration::from_secs(10), // TODO: Make this configurable
        )
    }

    /// Get the provider key that was assigned to this host @ startup
    pub fn provider_key(&self) -> &str {
        &self.provider_key
    }

    /// Stores actor with link definition
    pub async fn put_link(&self, ld: InterfaceLinkDefinition) {
        let mut update = self.links.write().await;
        update.insert(ld.source_id.to_string(), ld);
    }

    /// Deletes link
    pub async fn delete_link(&self, actor_id: &str) {
        let mut update = self.links.write().await;
        update.remove(actor_id);
    }

    /// Returns true if the actor is linked
    pub async fn is_linked(&self, actor_id: &str) -> bool {
        let read = self.links.read().await;
        read.contains_key(actor_id)
    }

    /// flush nats - called before main process exits
    pub(crate) async fn flush(&self) {
        if let Err(err) = self.nats.flush().await {
            error!(%err, "error flushing NATS client");
        }
    }

    /// Subscribe to a nats topic for rpc messages.
    /// This method starts a separate async task and returns immediately.
    /// It will exit if the nats client disconnects, or if a signal is received on the quit channel.
    pub async fn subscribe_rpc(
        &self,
        provider: impl WrpcDispatch + Clone + Send + 'static,
        quit: QuitSignal,
        lattice: String,
        provider_id: impl AsRef<str>,
    ) -> ProviderInitResult<Vec<JoinHandle<Result<()>>>> {
        let mut handles = Vec::new();
        let provider_id = provider_id.as_ref();

        // Build a wrpc client that we can use to listen for incoming invocations
        let wrpc_client = self.get_wrpc_client(provider_id);
        let link_name = self.link_name.clone();

        // For every mapping of world key names to dynamic functions to call, spawn a client that will listen
        // forever and process incoming invocations
        for (_nats_subject, (world_key_name, wit_fn, dyn_fn)) in
            self.incoming_invocation_fn_map.iter()
        {
            let wrpc_client = wrpc_client.clone();
            let world_key_name = world_key_name.clone();
            let wit_fn = wit_fn.clone();
            let lattice = lattice.clone();
            let provider = provider.clone();
            let fn_params = match dyn_fn {
                DynamicFunction::Method { params, .. } => params.clone(),
                DynamicFunction::Static { params, .. } => params.clone(),
            };
            let mut quit = quit.resubscribe();
            let this = self.clone();
            let provider_id = provider_id.to_string();
            let link_name = link_name.clone();

            trace!(
                "spawning invocation serving for [{}.{}]",
                world_key_name.as_str(),
                wit_fn.as_str()
            );

            // Set up stream of incoming invocations
            let mut invocations = wrpc_client
                .serve_dynamic(world_key_name.as_str(), wit_fn.as_str(), fn_params)
                .await
                .map_err(|e| {
                    ProviderInitError::Initialization(format!("failed to start wprc serving: {e}"))
                })?;

            // Spawn off process to handle invocations forever
            handles.push(spawn(async move {
                loop {
                    tokio::select! {
                        _ = quit.recv() => {
                            break Ok(());
                        }

                        invocation = invocations.try_next() => {
                            // Get the stream of invocations out
                            let AcceptedInvocation {
                                context,
                                params,
                                result_subject,
                                error_subject,
                                transmitter
                            } = match invocation {
                                Ok(Some(inv)) => inv,
                                // If we get an invocation that is empty, skip
                                Ok(None) => {
                                    continue;
                                },
                                // Process errors if we fail to get the invocation
                                Err(e) => {
                                    error!(error = %e, world_key_name, wit_fn, "failed to serve via wrpc");
                                    continue;
                                }
                            };

                            let invocation_id = Uuid::from_u128(Ulid::new().into()).to_string();
                            let operation = format!("{world_key_name}.{wit_fn}");
                            let current = tracing::Span::current();
                            let context = context.unwrap_or_default();
                            let source_id = context.get(WRPC_SOURCE_ID_HEADER_NAME)
                                .map(ToString::to_string)
                                .unwrap_or_else(|| "<unknown>".into());
                            current.record("operation", &tracing::field::display(&operation));
                            current.record("lattice_name", &tracing::field::display(&lattice));
                            current.record("invocation_id", &tracing::field::display(&invocation_id));
                            current.record("source_id", &tracing::field::display(&source_id));
                            current.record(
                                "host_id",
                                &tracing::field::display(&context.get(WRPC_HEADER_NAME_HOST_ID).map(ToString::to_string).unwrap_or("<unknown>".to_string()))
                            );
                            current.record("provider_id", provider_id.clone());
                            current.record("link_name", &tracing::field::display(&link_name));
                            let context = invocation_context(&context);

                            // Perform RPC
                            match this.handle_wrpc(provider.clone(), operation.clone(), source_id, params, context).in_current_span().await {
                                Ok(bytes) => {
                                    // Assuming that the provider has processed the request and produced objects
                                    // that conform to wrpc, transmit the response that were returned by the invocation
                                    if let Err(err) = transmitter.transmit(result_subject, bytes.into()).await {
                                        error!(%err, "failed to transmit invocation results");
                                    }
                                },
                                Err(err) => {
                                    error!(%err, %operation, "wRPC invocation failed");

                                    // Send the error forwards on the error subject
                                    if let Err(err) = transmitter
                                        .transmit_static(error_subject, format!("{err:#}"))
                                        .await
                                    {
                                        error!(?err, "failed to transmit error to invoker");
                                    }
                                },
                            };
                        }
                    }
                }
            }));
        }

        Ok(handles)
    }

    /// Handle an invocation coming from wRPC
    ///
    /// # Arguments
    ///
    /// * `provider` - The Provider
    /// * `operation` - The operation being performed (of the form `<ns>:<pkg>/<interface>.<function>`)
    /// * `source_id` - The ID of the origin which might represent one or more components/providers (ex. an actor public key)
    /// * `wrpc_invocation` - Details of the wRPC invocation
    async fn handle_wrpc(
        &self,
        provider: impl WrpcDispatch + 'static,
        operation: String,
        source_id: String,
        invocation_params: Vec<wrpc_transport::Value>,
        context: Context,
    ) -> InvocationResult<Vec<u8>> {
        // Dispatch the invocation to the provider
        let span = tracing::debug_span!("dispatch", %source_id, %operation);
        provider
            .dispatch_wrpc_dynamic(context, operation, invocation_params)
            .instrument(span)
            .await
    }
}
