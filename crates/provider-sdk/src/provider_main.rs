//! Functions for starting and running a provider

use std::io::BufRead;
use std::sync::Arc;

use base64::Engine;
use once_cell::sync::OnceCell;
use tokio::sync::broadcast;
use tokio::task::spawn_blocking;
use tracing::{error, info, instrument};
use wasmcloud_core::{HostData, InterfaceLinkDefinition};

use crate::error::{ProviderInitError, ProviderInitResult};
use crate::provider::ProviderConnection;
use crate::{with_connection_event_logging, Provider, DEFAULT_NATS_ADDR};

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

/// Starts a provider, reading all of the host data and starting the process
pub fn start_provider<P>(provider: P, friendly_name: &str) -> ProviderInitResult<()>
where
    P: Provider + Clone,
{
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

/// State of provider initialization
pub struct ProviderInitState {
    pub nats: async_nats::Client,
    pub shutdown_rx: broadcast::Receiver<()>,
    pub shutdown_tx: broadcast::Sender<()>,
    pub host_id: String,
    pub lattice_rpc_prefix: String,
    pub link_name: String,
    pub provider_key: String,
    pub link_definitions: Vec<InterfaceLinkDefinition>,
}

#[instrument]
pub async fn init_provider(name: &str) -> ProviderInitResult<ProviderInitState> {
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
        config_json: _,
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

    let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

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
    Ok(ProviderInitState {
        nats,
        shutdown_rx,
        shutdown_tx,
        host_id: host_id.clone(),
        lattice_rpc_prefix: lattice_rpc_prefix.clone(),
        link_name: link_name.clone(),
        provider_key: provider_key.clone(),
        link_definitions: link_definitions.clone(),
    })
}

/// Runs the provider. You can use this method instead of [`start_provider`] if you are already in
/// an async context
pub async fn run_provider<P>(provider: P, friendly_name: &str) -> ProviderInitResult<()>
where
    P: Provider + Clone,
{
    let ProviderInitState {
        nats,
        mut shutdown_rx,
        shutdown_tx,
        host_id,
        lattice_rpc_prefix,
        link_name,
        provider_key,
        link_definitions,
    } = init_provider(friendly_name).await?;

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
    )?;
    CONNECTION.set(connection).map_err(|_| {
        ProviderInitError::Initialization("Provider connection was already initialized".to_string())
    })?;
    let connection = get_connection();

    // pre-populate provider and bridge with initial set of link definitions
    // initialization of any link is fatal for provider startup
    for ld in link_definitions {
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
        .connect(provider, &shutdown_tx, &lattice_rpc_prefix)
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
