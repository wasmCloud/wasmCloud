//! Functions for starting and running a provider

use std::io::BufRead;
use std::str::FromStr;

use async_nats::{AuthError, ConnectOptions};
use base64::Engine;
use once_cell::sync::OnceCell;
use tracing::{error, info};

use crate::error::{ProviderInitError, ProviderInitResult};
use crate::provider::ProviderConnection;
use crate::Provider;

use wasmcloud_core::HostData;

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
pub fn start_provider<P>(provider: P, friendly_name: Option<String>) -> ProviderInitResult<()>
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

/// Runs the provider. You can use this method instead of [`start_provider`] if you are already in
/// an async context
pub async fn run_provider<P>(provider: P, friendly_name: Option<String>) -> ProviderInitResult<()>
where
    P: Provider + Clone,
{
    let host_data = tokio::task::spawn_blocking(load_host_data)
        .await
        .map_err(|e| {
            ProviderInitError::Initialization(format!("Unable to load host data: {e}"))
        })??;
    if let Err(e) = wasmcloud_tracing::configure_tracing(
        &friendly_name.unwrap_or(host_data.provider_key.clone()),
        &host_data.otel_config,
        host_data.structured_logging,
        host_data.log_level.as_ref(),
    ) {
        eprintln!("Failed to configure tracing: {e}");
    }

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
        ProviderInitError::Initialization(format!("Invalid nats server url '{nats_addr}': {e}"))
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
        ProviderInitError::Initialization("Provider connection was already initialized".to_string())
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
