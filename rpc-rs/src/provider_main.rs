#![cfg(not(target_arch = "wasm32"))]

use crate::{
    core::HostData,
    error::RpcError,
    provider::{HostBridge, ProviderDispatch},
    rpc_client::NatsClientType,
};
use once_cell::sync::OnceCell;

/// singleton host bridge for communicating with the host.
static BRIDGE: OnceCell<HostBridge> = OnceCell::new();

// this may be called any time after initialization
pub fn get_host_bridge() -> &'static HostBridge {
    match BRIDGE.get() {
        Some(b) => b,
        None => {
            // initialized first thing, so this shouldn't happen
            eprintln!("BRIDGE not initialized");
            panic!();
        }
    }
}

#[doc(hidden)]
/// Sets the bridge, return Err if it was already set
pub(crate) fn set_host_bridge(hb: HostBridge) -> Result<(), ()> {
    BRIDGE.set(hb).map_err(|_| ())
}

/// Start provider services: tokio runtime, logger, nats, and rpc subscriptions
pub fn provider_main<P>(provider_dispatch: P) -> Result<(), Box<dyn std::error::Error>>
where
    P: ProviderDispatch + Send + Sync + Clone + 'static,
{
    // get lattice configuration from host
    let host_data = load_host_data().map_err(|e| {
        eprintln!("error loading host data: {}", &e.to_string());
        Box::new(e)
    })?;
    provider_start(provider_dispatch, host_data)
}

/// Start provider services: tokio runtime, logger, nats, and rpc subscriptions,
pub fn provider_start<P>(
    provider_dispatch: P,
    host_data: HostData,
) -> Result<(), Box<dyn std::error::Error>>
where
    P: ProviderDispatch + Send + Sync + Clone + 'static,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        //.enable_io()
        .build()?;

    runtime.block_on(async { provider_run(provider_dispatch, host_data).await })?;
    // in the unlikely case there are any stuck threads,
    // close them so the process has a clean exit
    runtime.shutdown_timeout(core::time::Duration::from_secs(10));
    Ok(())
}

/// Async provider initialization
pub async fn provider_run<P>(
    provider_dispatch: P,
    host_data: HostData,
) -> Result<(), Box<dyn std::error::Error>>
where
    P: ProviderDispatch + Send + Sync + Clone + 'static,
{
    use std::str::FromStr as _;

    // initialize logger
    let log_rx = crate::channel_log::init_logger()
        .map_err(|_| RpcError::ProviderInit("log already initialized".to_string()))?;
    crate::channel_log::init_receiver(log_rx);

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    eprintln!(
        "Starting capability provider {} instance {} with nats url {}",
        &host_data.provider_key, &host_data.instance_id, &host_data.lattice_rpc_url,
    );

    let nats_addr = if !host_data.lattice_rpc_url.is_empty() {
        host_data.lattice_rpc_url.as_str()
    } else {
        crate::provider::DEFAULT_NATS_ADDR
    };
    let nats_server = nats_aflowt::ServerAddress::from_str(nats_addr).map_err(|e| {
        RpcError::InvalidParameter(format!("Invalid nats server url '{}': {}", nats_addr, e))
    })?;

    let nc = {
        cfg_if::cfg_if! {
            if #[cfg(feature="async_rewrite")] {

                NatsClientType::AsyncRewrite(nats_experimental::connect(nats_addr).await
                    .map_err(|e| {
                        RpcError::ProviderInit(format!("nats connection to {} failed: {}", nats_addr, e))
                    })?)

            } else {
                let nats_opts = match (
                    host_data.lattice_rpc_user_jwt.trim(),
                    host_data.lattice_rpc_user_seed.trim(),
                ) {
                    ("", "") => nats_aflowt::Options::default(),
                    (rpc_jwt, rpc_seed) => {
                        let kp = nkeys::KeyPair::from_seed(rpc_seed).unwrap();
                        let jwt = rpc_jwt.to_owned();
                        nats_aflowt::Options::with_jwt(
                            move || Ok(jwt.to_owned()),
                            move |nonce| kp.sign(nonce).unwrap(),
                        )
                    }
                };
                // Connect to nats
                NatsClientType::Async(nats_opts
                    .max_reconnects(None)
                    .connect(vec![nats_server])
                    .await
                    .map_err(|e| {
                        RpcError::ProviderInit(format!("nats connection to {} failed: {}", nats_addr, e))
                    })?)
            }
        }
    };

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
                "Error starting provider: failed to initialize link {:?}",
                &ld
            );
            return Err(Box::new(e));
        }
        bridge.put_link(ld).await;
    }

    // subscribe to nats topics
    let _join = bridge
        .connect(provider_dispatch, shutdown_tx)
        .await
        .map_err(|e| {
            RpcError::ProviderInit(format!("fatal error setting up subscriptions: {}", e))
        })?;

    // process subscription events and log messages, waiting for shutdown signal
    let _ = shutdown_rx.await;

    // close chunkifiers
    #[cfg(feature = "chunkify")]
    crate::chunkify::shutdown();

    // stop the logger thread
    //let _ = stop_log_thread.send(());
    crate::channel_log::stop_receiver();

    Ok(())
}

pub fn load_host_data() -> Result<HostData, RpcError> {
    use std::io::BufRead;

    let mut buffer = String::new();
    let stdin = std::io::stdin();
    {
        let mut handle = stdin.lock();
        handle.read_line(&mut buffer).map_err(|e| {
            RpcError::Rpc(format!(
                "failed to read host data configuration from stdin: {}",
                e
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
             {}",
            e
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
