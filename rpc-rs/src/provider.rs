#![cfg(not(target_arch = "wasm32"))]

//! common provider wasmbus support
//!

use crate::{
    core::{
        HealthCheckRequest, HealthCheckResponse, HostData, Invocation, InvocationResponse,
        LinkDefinition,
    },
    deserialize, serialize, Message, MessageDispatch, RpcError,
};
use anyhow::anyhow;
use log::{info, warn};
use nats::{Connection as NatsClient, Subscription};
use std::{
    borrow::Cow,
    collections::HashMap,
    convert::Infallible,
    ops::Deref,
    sync::{Arc, RwLock},
};
use tokio::{sync::oneshot, time::Duration};

const MAIN_LOOP_WAIT_INTERVAL: Duration = Duration::from_millis(50);

pub type HostShutdownEvent = String;

pub trait ProviderDispatch: MessageDispatch + ProviderHandler {}

pub mod prelude {
    pub use super::{ProviderDispatch, ProviderHandler};
    pub use crate::{client, context, Message, MessageDispatch, RpcError};

    //pub use crate::Timestamp;
    pub use async_trait::async_trait;
    pub use wasmbus_macros::Provider;

    #[cfg(feature = "BigInteger")]
    pub use num_bigint::BigInt as BigInteger;

    #[cfg(feature = "BigDecimal")]
    pub use bigdecimal::BigDecimal;
}

pub fn load_host_data() -> Result<HostData, anyhow::Error> {
    let mut buf = String::default();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
        .map_err(|e| anyhow!("failed to read host data configuration from stdin: {}", e))?;
    // remove spaces, tabs, and newlines before and after base64-encoded data
    let buf = buf.trim();
    if buf.is_empty() {
        return Err(anyhow!(
            "stdin is empty - expecting host data configuration"
        ));
    }
    let bytes = base64::decode(buf.as_bytes()).map_err(|e| {
        anyhow!(
            "host data configuration passed through stdin has invalid encoding (expected base64): {}",
            e
        )
    })?;
    let host_data: HostData =
        serde_json::from_slice(&bytes).map_err(|e| anyhow!("parsing host data: {}", e))?;
    Ok(host_data)
}

/// While waiting for messages, handle log messages sent to main thread.
/// Exits when shutdown_rx is received _or_ either channel is closed.
pub fn wait_for_shutdown(
    log_rx: crossbeam::channel::Receiver<LogEntry>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<HostShutdownEvent>,
) {
    use tokio::sync::oneshot::error::TryRecvError;

    loop {
        match log_rx.recv_timeout(MAIN_LOOP_WAIT_INTERVAL) {
            // if we have received a log message, continue
            // so that we check immediately - a group of messages that
            // arrive together will be printed with no delay between them.
            Ok((level, s)) => {
                log::logger().log(
                    &log::Record::builder()
                        .args(format_args!("{}", s))
                        .level(level)
                        .build(),
                );
                continue;
            }
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                warn!("Logger exited - quitting");
                break;
            }
        }

        // on each iteration of the loop, after all pending logs
        // have been written out, check for shutdown signal
        match shutdown_rx.try_recv() {
            Ok(_) => {
                info!("main thread received shutdown signal");
                // got the shutdown signal
                break;
            }
            Err(TryRecvError::Empty) => {
                // no signal yet, keep waiting
            }
            Err(TryRecvError::Closed) => {
                // sender exited
                break;
            }
        }
    }
}

/// CapabilityProvider handling of messages from host
/// The HostBridge handles most messages and forwards the remainder to this handler
pub trait ProviderHandler: Sync {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    /// This message is idempotent - provider must be able to handle
    /// duplicates
    #[allow(unused_variables)]
    fn put_link(&self, ld: &LinkDefinition) -> Result<bool, RpcError> {
        Ok(true)
    }

    /// Notify the provider that the link is dropped
    #[allow(unused_variables)]
    fn delete_link(&self, actor_id: &str) {}

    /// Perform health check. Called at regular intervals by host
    fn health_request(&self, arg: &HealthCheckRequest) -> Result<HealthCheckResponse, RpcError>;

    /// Handle system shutdown message
    fn shutdown(&self) -> Result<(), Infallible> {
        Ok(())
    }
}

/// format of log message sent to main thread for output to logger
pub type LogEntry = (log::Level, String);

/// HostBridge manages the NATS connection to the host,
/// and processes subscriptions for links, health-checks, and rpc messages.
/// Callbacks from HostBridge are implemented by the provider in the [[ProviderHandler]] implementation.
///
/// HostBridge subscribes to these nats subscriptions on behalf of the provider
/// - wasmbus.rpc.{prefix}.{provider_key}.{link_name}.rpc - Get Invocation, answer InvocationResponse
/// - wasmbus.rpc.{prefix}.{provider_key}.{link_name}.health - Health check
/// - wasmbus.rpc.{prefix}.{provider_key}.{link_name}.shutdown - Request for graceful shutdown
/// - wasmbus.rpc.{prefix}.{provider_key}.{link_name}.linkdefs.del - Remove a link def. Provider de-provisions resources for the given actor.
/// - wasmbus.rpc.{prefix}.{provider_key}.{link_name}.linkdefs.put - Puts a link def. Provider provisions resources for the given actor.
///
#[derive(Clone)]
pub struct HostBridge {
    inner: Arc<HostBridgeInner>,
}

impl HostBridge {
    pub fn new(
        nats: NatsClient,
        host_data: &HostData,
        log_tx: crossbeam::channel::Sender<LogEntry>,
    ) -> HostBridge {
        HostBridge {
            inner: Arc::new(HostBridgeInner {
                nats,
                log_tx,
                subs: RwLock::new(Vec::new()),
                links: RwLock::new(HashMap::new()),
                lattice_prefix: host_data.lattice_rpc_prefix.clone(),
            }),
        }
    }
}

impl Deref for HostBridge {
    type Target = HostBridgeInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[doc(hidden)]
pub struct HostBridgeInner {
    subs: RwLock<Vec<Subscription>>,
    /// Table of actors that are bound to this provider
    /// Key is actor_id / actor public key
    links: RwLock<HashMap<String, LinkDefinition>>,
    nats: NatsClient,
    log_tx: crossbeam::channel::Sender<LogEntry>,
    lattice_prefix: String,
}

impl HostBridge {
    /// Clear out all subscriptions
    pub fn unsubscribe_all(&self) {
        // make sure we call drain on all subscriptions
        for sub in self.subs.read().unwrap().iter() {
            let _ = sub.drain();
        }
    }

    /// Send log to main thread
    fn log(&self, level: log::Level, s: String) {
        let _ = self.log_tx.send((level, s));
    }

    /// Stores actor with link definition
    pub fn put_link(&self, ld: LinkDefinition) {
        self.links
            .write()
            .unwrap()
            .insert(ld.actor_id.to_string(), ld);
    }

    /// Deletes link
    pub fn delete_link(&self, actor_id: &str) {
        self.links.write().unwrap().remove(actor_id);
    }

    /// Returns true if the actor is linked
    pub fn is_linked(&self, actor_id: &str) -> bool {
        self.links.read().unwrap().contains_key(actor_id)
    }

    /// Returns copy of LinkDefinition, or None,if the actor is not linked
    pub fn get_link(&self, actor_id: &str) -> Option<LinkDefinition> {
        self.links.read().unwrap().get(actor_id).cloned()
    }

    /// Implement subscriber listener threads and provider callbacks
    pub fn connect<P>(
        &'static self,
        provider_key: String,
        link_name: &str,
        provider_impl: P,
        shutdown_tx: oneshot::Sender<HostShutdownEvent>,
    ) -> Result<(), anyhow::Error>
    where
        P: ProviderDispatch + Send + Sync,
        P: 'static,
        P: Clone,
    {
        use std::fmt;

        // send all logging to main thread. This is intentionally blocking
        // (channel size=0) so that logs are "immediate"
        macro_rules! debug {
            ($($arg:tt)*) => ({ self.log(log::Level::Debug, fmt::format(format_args!($($arg)*))); })
        }
        macro_rules! warn {
            ($($arg:tt)*) => ({ self.log(log::Level::Warn, fmt::format(format_args!($($arg)*))); })
        }
        macro_rules! info {
            ($($arg:tt)*) => ({ self.log(log::Level::Info, fmt::format(format_args!($($arg)*))); })
        }
        macro_rules! error {
            ($($arg:tt)*) => ({ self.log(log::Level::Error, fmt::format(format_args!($($arg)*))); })
        }
        let nats = self.nats.clone();

        // Link Delete
        let link_del_topic = format!(
            "wasmbus.rpc.{}.{}.{}.linkdefs.del",
            &self.lattice_prefix, &provider_key, &link_name
        );
        let sub = nats.subscribe(&link_del_topic)?;
        self.subs.write().unwrap().push(sub.clone());
        let (this, provider) = (self.clone(), provider_impl.clone());
        tokio::task::spawn(async move {
            for msg in sub.iter() {
                match deserialize::<LinkDefinition>(&msg.data) {
                    Ok(ld) => {
                        this.delete_link(&ld.actor_id);
                        // notify provider that link is deleted
                        provider.delete_link(&ld.actor_id);
                    }
                    Err(e) => {
                        error!(
                            "error deserializing link-delete parameter: {}",
                            e.to_string()
                        );
                    }
                }
            }
        });

        // Link Put
        let ldput_topic = format!(
            "wasmbus.rpc.{}.{}.{}.linkdefs.put",
            &self.lattice_prefix, &provider_key, &link_name
        );
        let sub = nats.subscribe(&ldput_topic)?;
        self.subs.write().unwrap().push(sub.clone());
        let (this, provider) = (self.clone(), provider_impl.clone());
        tokio::task::spawn(async move {
            for msg in sub.iter() {
                match deserialize::<LinkDefinition>(&msg.data) {
                    Ok(ld) => {
                        if this.is_linked(&ld.actor_id) {
                            // if already linked, print warning
                            warn!(
                                "Received LD put for existing link definition from {} to {}\n",
                                &ld.actor_id, &ld.provider_id
                            );
                        }
                        match provider.put_link(&ld) {
                            Ok(true) => {
                                info!("Linking {}\n", &ld.actor_id);
                                this.put_link(ld);
                            }
                            Ok(false) => {
                                // authorization failed or parameters were invalid
                                warn!("put_link denied: {}\n", &ld.actor_id);
                            }
                            Err(e) => {
                                error!("put_link {} failed: {}\n", &ld.actor_id, e.to_string());
                            }
                        }
                    }
                    Err(e) => {
                        error!("error serializing put-link parameter: {}", e.to_string());
                    }
                }
            }
        });

        // Shutdown
        let shutdown_topic = format!(
            "wasmbus.rpc.{}.{}.{}.shutdown",
            &self.lattice_prefix, &provider_key, link_name
        );
        let sub = nats.subscribe(&shutdown_topic)?;
        // don't add this sub to subs list
        let (this, provider) = (self.clone(), provider_impl.clone());
        tokio::task::spawn(async move {
            if let Some(msg) = sub.next() {
                info!("Received termination signal. Shutting down capability provider.");
                // tell provider to shutdown - before we shut down nats subscriptions
                let _ = provider.shutdown();
                // drain all subscriptions (other than this one)
                for sub in this.subs.read().unwrap().iter() {
                    let _ = sub.drain();
                }
                // send ack to host
                let _ = msg.respond("Shutting down");
                // signal main thread that we are ready
                if let Err(e) = shutdown_tx.send("bye".to_string()) {
                    error!("Problem shutting down:  failure to send signal: {}\n", e);
                }
            }
            // if subh.next() returns None, subscription has been cancelled or the connection was dropped.
            // in either case, fall through to exit thread
        });

        // Health check
        let health_topic = format!("wasmbus.rpc.{}.{}.health", &provider_key, &link_name);
        let sub = nats.subscribe(&health_topic)?;
        self.subs.write().unwrap().push(sub.clone());
        let provider = provider_impl.clone();
        tokio::task::spawn(async move {
            for msg in sub.iter() {
                debug!("received health check request\n");
                // placeholder arg
                let arg = HealthCheckRequest {};
                let resp = match provider.health_request(&arg) {
                    Ok(resp) => resp,
                    Err(e) => {
                        error!("error generating health check response: {}", &e.to_string());
                        HealthCheckResponse {
                            healthy: false,
                            message: Some(e.to_string()),
                        }
                    }
                };
                match serialize(&resp) {
                    Ok(t) => {
                        if let Err(e) = msg.respond(t) {
                            error!("failed sending health check response: {}", e.to_string());
                        }
                    }
                    Err(e) => {
                        // extremely unlikely that InvocationResponse would fail to serialize
                        error!("failed serializing HealthCheckResponse: {}", e.to_string());
                    }
                }
            }
        });

        // RPC messages
        //
        let rpc_topic = format!(
            "wasmbus.rpc.{}.{}.{}",
            &self.lattice_prefix, &provider_key, link_name
        );
        let sub = nats.subscribe(&rpc_topic)?;
        self.subs.write().unwrap().push(sub.clone());
        tokio::task::spawn(async move {
            let provider = provider_impl.clone();
            let ctx = crate::context::Context::default();
            for msg in sub.iter() {
                debug!("received an RPC message");
                let inv = match deserialize::<Invocation>(&msg.data) {
                    Ok(inv) => {
                        debug!(
                            "Received RPC Invocation: op:{} from:{}\n",
                            &inv.operation, &inv.origin.public_key,
                        );
                        inv
                    }
                    Err(e) => {
                        error!("received corrupt invocation: {}\n", e.to_string());
                        continue;
                    }
                };
                let provider_msg = Message {
                    method: &inv.operation,
                    arg: Cow::from(inv.msg),
                };
                let ir = match provider.dispatch(&ctx, provider_msg).await {
                    Ok(resp) => {
                        debug!("operation {} succeeded. returning response", &inv.operation);
                        InvocationResponse {
                            msg: resp.arg.to_vec(),
                            error: None,
                            invocation_id: inv.id,
                        }
                    }
                    Err(e) => {
                        error!(
                            "operation {} failed with error {}\n",
                            &inv.operation,
                            e.to_string()
                        );
                        InvocationResponse {
                            msg: Vec::new(),
                            error: Some(e.to_string()),
                            invocation_id: inv.id,
                        }
                    }
                };
                match serialize(&ir) {
                    Ok(t) => {
                        if let Err(e) = msg.respond(t) {
                            error!(
                                "failed sending response to op {}: {}",
                                &inv.operation,
                                e.to_string()
                            );
                        }
                    }
                    Err(e) => {
                        // extremely unlikely that InvocationResponse would fail to serialize
                        error!(
                            "failed serializing InvocationResponse to op:{} : {}\n",
                            &inv.operation,
                            e.to_string()
                        );
                    }
                }
            }
        });
        Ok(())
    }
}
