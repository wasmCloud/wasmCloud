use core::fmt::{self, Formatter};
use core::future::Future;
use core::pin::{pin, Pin};
use std::{collections::HashMap, io::BufRead, sync::Arc, time::Duration};

use anyhow::Context as _;
use async_nats::{ConnectOptions, Event, HeaderMap};
use base64::Engine;
use bytes::Bytes;
use futures::{stream, Stream, StreamExt as _, TryStreamExt};
use nkeys::XKey;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use tokio::{
    select,
    sync::broadcast,
    task::{spawn_blocking, JoinSet},
};
use tracing::{debug, error, info, trace, warn};
use wasmcloud_core::ExtensionData;
use wrpc_transport::InvokeExt as _;

use wasmcloud_core::nats::convert_header_map_to_hashmap;
#[cfg(feature = "otel")]
use wasmcloud_core::TraceContext;
#[cfg(feature = "otel")]
use wasmcloud_tracing::context::attach_span_context;

use crate::error::{ProviderInitError, ProviderInitResult};

// pub(crate) static CONNECTION: OnceCell<ProviderConnection> = OnceCell::new();
pub(crate) static EXT_DATA: OnceCell<ExtensionData> = OnceCell::new();
static CONNECTION: OnceCell<ProviderConnection> = OnceCell::new();

/// Name of the header that should be passed for invocations that identifies the source
const WRPC_SOURCE_ID_HEADER_NAME: &str = "source-id";

/// nats address to use if not included in initial `HostData`
pub(crate) const DEFAULT_NATS_ADDR: &str = "nats://127.0.0.1:4222";

/// Retrieves the currently configured connection to the lattice. DO NOT call this method until
/// after the provider is running (meaning [`run_provider`] has been called)
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
pub fn load_ext_data() -> ProviderInitResult<&'static ExtensionData> {
    EXT_DATA.get_or_try_init(_load_ext_data)
}

// Internal function for populating the extension data.
// Tries stdin first (for internal/managed providers spawned by host),
// then falls back to environment variables (for external/standalone providers).
fn _load_ext_data() -> ProviderInitResult<ExtensionData> {
    // First, attempt to load from stdin (non-blocking check)
    // Internal providers spawned by the host receive ExtensionData via stdin
    if let Some(ext_data) = try_load_from_stdin() {
        return Ok(ext_data);
    }

    // Fall back to environment variables for external providers
    ExtensionData::from_env().map_err(ProviderInitError::Initialization)
}

/// Try to load ExtensionData from stdin (base64-encoded JSON).
/// Returns None if stdin is empty or not available, Some(ExtensionData) on success.
fn try_load_from_stdin() -> Option<ExtensionData> {
    use std::io::IsTerminal;

    // If stdin is a terminal, skip trying to read from it
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }

    let mut buffer = String::new();
    {
        let mut handle = stdin.lock();
        if handle.read_line(&mut buffer).is_err() {
            return None;
        }
    }

    let buffer = buffer.trim();
    if buffer.is_empty() {
        return None;
    }

    // Try to decode base64
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(buffer.as_bytes())
        .ok()?;

    // Try to parse as ExtensionData
    serde_json::from_slice(&bytes).ok()
}

#[derive(Clone)]
pub struct ProviderConnection {
    /// NATS client used for performing RPCs
    pub nats: Arc<async_nats::Client>,

    /// Lattice name
    pub lattice: Arc<str>,
    pub provider_id: Arc<str>,

    /// Host ID this provider is bound to (for extension interface routing)
    pub host_id: Arc<str>,

    /// Secrets XKeys
    pub provider_xkey: Arc<XKey>,
}

impl fmt::Debug for ProviderConnection {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConnection")
            .field("provider_id", &self.provider_id)
            .field("lattice", &self.lattice)
            .finish()
    }
}

impl ProviderConnection {
    pub fn new(
        nats: impl Into<Arc<async_nats::Client>>,
        provider_id: impl Into<Arc<str>>,
        lattice: impl Into<Arc<str>>,
        host_id: impl Into<Arc<str>>,
    ) -> ProviderInitResult<ProviderConnection> {
        // Generate XKey internally inside the provider
        let provider_private_xkey = XKey::new();
        Ok(ProviderConnection {
            nats: nats.into(),
            lattice: lattice.into(),
            provider_id: provider_id.into(),
            host_id: host_id.into(),
            provider_xkey: provider_private_xkey.into(),
        })
    }

    /// Retrieve a wRPC client that can be used based on the NATS client of this connection
    ///
    /// # Arguments
    ///
    /// * `target` - Target ID to which invocations will be sent
    pub async fn get_wrpc_client(&self, target: &str) -> anyhow::Result<WrpcClient> {
        self.get_wrpc_client_custom(target, None).await
    }

    /// Retrieve a wRPC client that can be used based on the NATS client of this connection,
    /// customized with invocation timeout
    ///
    /// # Arguments
    ///
    /// * `target` - Target ID to which invocations will be sent
    /// * `timeout` - Timeout to be set on the client (by default if this is unset it will be 10 seconds)
    pub async fn get_wrpc_client_custom(
        &self,
        target: &str,
        timeout: Option<Duration>,
    ) -> anyhow::Result<WrpcClient> {
        let prefix = Arc::from(format!("{}.{target}", &self.lattice));
        let nats = wrpc_transport_nats::Client::new(
            Arc::clone(&self.nats),
            Arc::clone(&prefix),
            Some(prefix),
        )
        .await?;
        Ok(WrpcClient {
            nats,
            provider_id: Arc::clone(&self.provider_id),
            target: Arc::from(target),
            timeout: timeout.unwrap_or_else(|| Duration::from_secs(10)),
        })
    }

    /// Retrieve a wRPC client for serving extension interfaces (manageable, configurable).
    /// Extension interfaces are served on a host-specific subject to enable targeted management.
    ///
    /// Subject format: `wasmbus.ctl.v1.{lattice}.extension.{provider_id}.{host_id}`
    ///
    /// This follows the existing wasmCloud ctl subject pattern for consistency.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Timeout to be set on the client (by default if this is unset it will be 10 seconds)
    pub async fn get_wrpc_extension_serve_client_custom(
        &self,
        timeout: Option<Duration>,
    ) -> anyhow::Result<WrpcClient> {
        // Extension interfaces use ctl subject pattern for host-specific targeted management
        let prefix = Arc::from(format!(
            "wasmbus.ctl.v1.{}.extension.{}.{}",
            &self.lattice, &self.provider_id, &self.host_id
        ));
        let nats = wrpc_transport_nats::Client::new(
            Arc::clone(&self.nats),
            Arc::clone(&prefix),
            Some(prefix),
        )
        .await?;
        Ok(WrpcClient {
            nats,
            provider_id: Arc::clone(&self.provider_id),
            target: Arc::clone(&self.provider_id),
            timeout: timeout.unwrap_or_else(|| Duration::from_secs(10)),
        })
    }

    /// Get the provider key that was assigned to this host at startup
    #[must_use]
    pub fn provider_key(&self) -> &str {
        &self.provider_id
    }

    /// Create both wRPC clients needed for serving provider exports.
    /// Returns (main_client, extension_client).
    pub async fn get_wrpc_clients_for_serving(&self) -> anyhow::Result<(WrpcClient, WrpcClient)> {
        let main_client = self
            .get_wrpc_client(&self.provider_id)
            .await
            .context("failed to create main wRPC client")?;
        let extension_client = self
            .get_wrpc_extension_serve_client_custom(None)
            .await
            .context("failed to create extension wRPC client")?;
        Ok((main_client, extension_client))
    }

    /// flush nats - called before main process exits
    pub(crate) async fn flush(&self) {
        if let Err(err) = self.nats.flush().await {
            error!(%err, "error flushing NATS client");
        }
    }
}

/// helper method to add logging to a nats connection. Logs disconnection (warn level), reconnection (info level), error (error), slow consumer, and lame duck(warn) events.
#[must_use]
pub fn with_connection_event_logging(opts: ConnectOptions) -> ConnectOptions {
    opts.event_callback(|event| async move {
        match event {
            Event::Connected => info!("nats client connected"),
            Event::Disconnected => warn!("nats client disconnected"),
            Event::Draining => warn!("nats client draining"),
            Event::LameDuckMode => warn!("nats lame duck mode"),
            Event::SlowConsumer(val) => warn!("nats slow consumer detected ({val})"),
            Event::ClientError(err) => error!("nats client error: '{err:?}'"),
            Event::ServerError(err) => error!("nats server error: '{err:?}'"),
            Event::Closed => error!("nats client closed"),
        }
    })
}

#[derive(Clone)]
pub struct WrpcClient {
    nats: wrpc_transport_nats::Client,
    timeout: Duration,
    provider_id: Arc<str>,
    target: Arc<str>,
}

impl wrpc_transport::Invoke for WrpcClient {
    type Context = Option<HeaderMap>;
    type Outgoing = <wrpc_transport_nats::Client as wrpc_transport::Invoke>::Outgoing;
    type Incoming = <wrpc_transport_nats::Client as wrpc_transport::Invoke>::Incoming;

    async fn invoke<P>(
        &self,
        cx: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let mut headers = cx.unwrap_or_default();
        headers.insert("source-id", &*self.provider_id);
        headers.insert("target-id", &*self.target);
        self.nats
            .timeout(self.timeout)
            .invoke(Some(headers), instance, func, params, paths)
            .await
    }
}

impl wrpc_transport::Serve for WrpcClient {
    type Context = Option<Context>;
    type Outgoing = <wrpc_transport_nats::Client as wrpc_transport::Serve>::Outgoing;
    type Incoming = <wrpc_transport_nats::Client as wrpc_transport::Serve>::Incoming;

    async fn serve(
        &self,
        instance: &str,
        func: &str,
        paths: impl Into<Arc<[Box<[Option<usize>]>]>> + Send,
    ) -> anyhow::Result<
        impl Stream<Item = anyhow::Result<(Self::Context, Self::Outgoing, Self::Incoming)>>
            + Send
            + 'static,
    > {
        let invocations = self.nats.serve(instance, func, paths).await?;
        Ok(invocations.and_then(|(cx, tx, rx)| async move {
            Ok((cx.as_ref().map(invocation_context), tx, rx))
        }))
    }
}

/// Extracts trace context from incoming headers
fn invocation_context(headers: &HeaderMap) -> Context {
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
        .map_or_else(|| "<unknown>".into(), ToString::to_string);
    Context {
        component: Some(source_id),
        tracing: convert_header_map_to_hashmap(headers),
    }
}

/// Context - message passing metadata used by wasmCloud Capability Providers
#[derive(Default, Debug, Clone)]
pub struct Context {
    /// Messages received by a Provider will have component set to the component's ID
    pub component: Option<String>,

    /// A map of tracing context information
    pub tracing: HashMap<String, String>,
}

impl Context {
    /// Get link name from the request.
    ///
    /// While link name should in theory *always* be present, it is not natively included in [`Context`] yet,
    /// so we must retrieve it from headers on the request.
    ///
    /// Note that in certain (older) versions of wasmCloud it is possible for the link name to be missing
    /// though incredibly unlikely (basically, due to a bug). In the event that the link name was *not*
    /// properly stored on the context 'default' (the default link name) is returned as the link name.
    #[must_use]
    pub fn link_name(&self) -> &str {
        self.tracing
            .get("link-name")
            .map_or("default", String::as_str)
    }
}

/// Handle shutdown for providers.
/// Shutdown can be triggered via:
/// - wRPC manageable::Handler::shutdown() method (signals quit channel)
/// - OS signals (SIGTERM, SIGINT/Ctrl-C) for standalone providers
async fn handle_provider_shutdown(
    connection: &ProviderConnection,
    mut quit_rx: broadcast::Receiver<()>,
    quit_tx: broadcast::Sender<()>,
) {
    #[cfg(unix)]
    {
        use tokio::signal;

        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to setup SIGTERM handler");
        let mut sigint = signal::unix::signal(signal::unix::SignalKind::interrupt())
            .expect("failed to setup SIGINT handler");

        select! {
            // Wait for quit signal from wRPC shutdown or other internal trigger
            _ = quit_rx.recv() => {
                info!("Received shutdown signal, shutting down provider");
            }
            // Wait for SIGTERM
            _ = sigterm.recv() => {
                info!("Received SIGTERM, shutting down provider");
                let _ = quit_tx.send(());
            }
            // Wait for SIGINT/Ctrl-C
            _ = sigint.recv() => {
                info!("Received SIGINT (Ctrl-C), shutting down provider");
                let _ = quit_tx.send(());
            }
        }
    }

    #[cfg(not(unix))]
    {
        use tokio::signal;

        select! {
            _ = quit_rx.recv() => {
                info!("Received shutdown signal, shutting down provider");
            }
            _ = signal::ctrl_c() => {
                info!("Received Ctrl-C, shutting down provider");
                let _ = quit_tx.send(());
            }
        }
    }

    // Flush NATS client before shutdown
    connection.flush().await;
}

/// Called when initializing the provider handler given a provider name and optionally providing host data if not apart of a host already.
/// This happens during startup of the provider itself.
/// It returns a tuple of:
/// - A [Future] which will become ready once shutdown signal is received
/// - A [broadcast::Sender] that can be used to trigger shutdown (e.g., from wRPC manageable::Handler::shutdown())
pub async fn run_provider(
    friendly_name: &str,
    ext_data: Option<&ExtensionData>,
) -> ProviderInitResult<(impl Future<Output = ()>, broadcast::Sender<()>)> {
    let ext_data = match ext_data {
        Some(hd) => hd,
        None => spawn_blocking(load_ext_data).await.map_err(|e| {
            ProviderInitError::Initialization(format!("failed to load host data: {e}"))
        })??,
    };

    let ExtensionData {
        lattice_rpc_prefix,
        lattice_rpc_user_jwt,
        lattice_rpc_user_seed,
        lattice_rpc_url,
        provider_id,
        instance_id,
        host_id,
        default_rpc_timeout_ms: _,
        ..
    } = ext_data;

    info!(
        "Starting capability provider {provider_id} instance {instance_id} with nats url {lattice_rpc_url}"
    );

    // Build the NATS client
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
    .name(friendly_name)
    .connect(nats_addr)
    .await?;
    let nats = Arc::new(nats);

    // Create quit channel for shutdown coordination
    // Providers should call quit_tx.send(()) when they receive the wRPC shutdown request
    let (quit_tx, quit_rx) = broadcast::channel(1);

    let connection = ProviderConnection::new(
        Arc::clone(&nats),
        provider_id.as_str(),
        lattice_rpc_prefix.as_str(),
        host_id.as_str(),
    )?;

    CONNECTION.set(connection).map_err(|_| {
        ProviderInitError::Initialization("Provider connection was already initialized".to_string())
    })?;
    let connection = get_connection();

    debug!(?friendly_name, "provider finished initialization");

    let quit_tx_clone = quit_tx.clone();
    Ok((
        handle_provider_shutdown(connection, quit_rx, quit_tx_clone),
        quit_tx,
    ))
}

/// This is the type returned by the `serve` function generated by [`wit-bindgen-wrpc`]
pub type InvocationStreams = Vec<(
    &'static str,
    &'static str,
    Pin<
        Box<
            dyn Stream<
                    Item = anyhow::Result<
                        Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>,
                    >,
                > + Send
                + 'static,
        >,
    >,
)>;

/// Default timeout for graceful shutdown - wait for in-flight tasks to complete
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Serve exports of the provider using the `serve` function generated by [`wit-bindgen-wrpc`]
///
/// This function serves both the main provider exports (on the standard lattice subject)
/// and the extension capability interface (on the host-specific extension subject).
///
/// # Arguments
/// * `main_client` - wRPC client for main provider exports (standard lattice subject)
/// * `extension_client` - wRPC client for extension interfaces (host-specific subject)
/// * `provider` - The provider implementation (must be Clone to serve on both interfaces)
/// * `shutdown` - A future that resolves when shutdown is requested
/// * `serve` - The main serve function for provider exports
/// * `serve_extension` - The serve function for extension interfaces
pub async fn serve_provider_exports<'a, P, F1, F2, Fut1, Fut2>(
    main_client: &'a WrpcClient,
    extension_client: &'a WrpcClient,
    provider: P,
    shutdown: impl Future<Output = ()>,
    serve: F1,
    serve_extension: F2,
) -> anyhow::Result<()>
where
    P: Clone,
    F1: FnOnce(&'a WrpcClient, P) -> Fut1,
    F2: FnOnce(&'a WrpcClient, P) -> Fut2,
    Fut1: Future<Output = anyhow::Result<InvocationStreams>> + wrpc_transport::Captures<'a>,
    Fut2: Future<Output = anyhow::Result<InvocationStreams>> + wrpc_transport::Captures<'a>,
{
    // Serve main provider exports on the standard lattice subject
    let main_invocations = serve(main_client, provider.clone())
        .await
        .context("failed to serve main provider exports")?;

    // Serve extension capability interface on the host-specific subject
    let extension_invocations = serve_extension(extension_client, provider)
        .await
        .context("failed to serve extension capability interface")?;

    // Helper function to map invocation streams
    fn map_invocation_stream(
        (instance, name, invocations): (
            &'static str,
            &'static str,
            Pin<
                Box<
                    dyn Stream<
                            Item = anyhow::Result<
                                Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>,
                            >,
                        > + Send
                        + 'static,
                >,
            >,
        ),
    ) -> impl Stream<
        Item = (
            &'static str,
            &'static str,
            anyhow::Result<Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>>,
        ),
    > {
        invocations.map(move |res| (instance, name, res))
    }

    // Combine both invocation streams into a single stream
    let main_streams = main_invocations.into_iter().map(map_invocation_stream);
    let extension_streams = extension_invocations.into_iter().map(map_invocation_stream);

    let mut invocations = stream::select_all(main_streams.chain(extension_streams));
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();

    loop {
        select! {
            Some((instance, name, res)) = invocations.next() => {
                match res {
                    Ok(fut) => {
                        tasks.spawn(async move {
                            if let Err(err) = fut.await {
                                warn!(?err, instance, name, "failed to serve invocation");
                            }
                            trace!(instance, name, "successfully served invocation");
                        });
                    },
                    Err(err) => {
                        warn!(?err, instance, name, "failed to accept invocation");
                    }
                }
            },
            () = &mut shutdown => {
                // Graceful shutdown: stop accepting new invocations, wait for in-flight to complete
                let task_count = tasks.len();
                if task_count > 0 {
                    info!("Shutdown requested, waiting for {} in-flight tasks", task_count);

                    // Wait for tasks with a timeout
                    let shutdown_result = tokio::time::timeout(
                        GRACEFUL_SHUTDOWN_TIMEOUT,
                        async {
                            while tasks.join_next().await.is_some() {}
                        }
                    ).await;

                    match shutdown_result {
                        Ok(()) => {
                            info!("Shutdown completed gracefully");
                        }
                        Err(_) => {
                            let remaining = tasks.len();
                            warn!(remaining, "Graceful shutdown timeout, aborting remaining tasks");
                            tasks.abort_all();
                            while tasks.join_next().await.is_some() {}
                        }
                    }
                }

                info!("Provider shutdown complete");
                return Ok(())
            }
        }
    }
}

/// Serve extension interfaces of a provider using the `serve` function generated by [`wit-bindgen-wrpc`]
///
/// This function is a simplified version of `serve_provider_exports` for providers
/// that only expose extension interfaces to the host (e.g., built-in providers).
///
/// # Arguments
/// * `extension_client` - wRPC client for extension interfaces (host-specific subject)
/// * `provider` - The provider implementation
/// * `shutdown` - A future that resolves when shutdown is requested
/// * `serve` - The `serve` function for all extension interfaces, which takes a wRPC client for the host-specific subject
pub async fn serve_provider_extension<'a, P, F, Fut>(
    extension_client: &'a WrpcClient,
    provider: P,
    shutdown: impl Future<Output = ()>,
    serve: F,
) -> anyhow::Result<()>
where
    P: Clone,
    F: FnOnce(&'a WrpcClient, P) -> Fut,
    Fut: Future<Output = anyhow::Result<InvocationStreams>> + wrpc_transport::Captures<'a>,
{
    // Serve extension capability interface on the host-specific subject
    let extension_invocations = serve(extension_client, provider)
        .await
        .context("failed to serve extension capability interface")?;

    // Helper function to map invocation streams
    fn map_invocation_stream(
        (instance, name, invocations): (
            &'static str,
            &'static str,
            Pin<
                Box<
                    dyn Stream<
                            Item = anyhow::Result<
                                Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>,
                            >,
                        > + Send
                        + 'static,
                >,
            >,
        ),
    ) -> impl Stream<
        Item = (
            &'static str,
            &'static str,
            anyhow::Result<Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>>,
        ),
    > {
        invocations.map(move |res| (instance, name, res))
    }

    let mut invocations =
        stream::select_all(extension_invocations.into_iter().map(map_invocation_stream));
    let mut shutdown = pin!(shutdown);
    let mut tasks = JoinSet::new();

    loop {
        select! {
            Some((instance, name, res)) = invocations.next() => {
                match res {
                    Ok(fut) => {
                        tasks.spawn(async move {
                            if let Err(err) = fut.await {
                                warn!(?err, instance, name, "failed to serve invocation");
                            }
                            trace!(instance, name, "successfully served invocation");
                        });
                    },
                    Err(err) => {
                        warn!(?err, instance, name, "failed to accept invocation");
                    }
                }
            },
            () = &mut shutdown => {
                // Graceful shutdown: stop accepting new invocations, wait for in-flight to complete
                let task_count = tasks.len();
                if task_count > 0 {
                    info!("Shutdown requested, waiting for {} in-flight tasks", task_count);

                    // Wait for tasks with a timeout
                    let shutdown_result = tokio::time::timeout(
                        GRACEFUL_SHUTDOWN_TIMEOUT,
                        async {
                            while tasks.join_next().await.is_some() {}
                        }
                    ).await;

                    match shutdown_result {
                        Ok(()) => {
                            info!("Shutdown completed gracefully");
                        }
                        Err(_) => {
                            let remaining = tasks.len();
                            warn!(remaining, "Graceful shutdown timeout, aborting remaining tasks");
                            tasks.abort_all();
                            while tasks.join_next().await.is_some() {}
                        }
                    }
                }

                info!("Provider shutdown complete");
                return Ok(())
            }
        }
    }
}
