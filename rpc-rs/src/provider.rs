#![cfg(not(target_arch = "wasm32"))]

//! common provider wasmbus support
//!

use crate::{
    core::{
        HealthCheckRequest, HealthCheckResponse, HostData, Invocation, InvocationResponse,
        LinkDefinition, WasmCloudEntity,
    },
    Message, MessageDispatch, RpcError,
};
use async_trait::async_trait;
use futures::{future::JoinAll, StreamExt};
use log::{debug, error, info, trace, warn};
use pin_utils::pin_mut;
// re-export ratsio crate for simpler version compatibility
pub use ratsio::{self, NatsClient};
// re-export make_uuid
pub use crate::rpc_client::make_uuid;
use serde::de::DeserializeOwned;
use std::{borrow::Cow, collections::HashMap, convert::Infallible, ops::Deref, sync::Arc};
use tokio::sync::{oneshot, RwLock};

// name of nats queue group for rpc subscription
const RPC_SUBSCRIPTION_QUEUE_GROUP: &str = "rpc";

type SubscriptionId = ratsio::NatsSid;

pub type HostShutdownEvent = String;

pub trait Subscription: futures::stream::Stream<Item = ratsio::NatsMessage> + Send + Sync {}

pub trait ProviderDispatch: MessageDispatch + ProviderHandler {}
trait ProviderImpl: ProviderDispatch + Send + Sync + Clone + 'static {}

pub mod prelude {
    pub use crate::provider::{HostBridge, NatsClient, ProviderDispatch, ProviderHandler};
    pub use crate::{
        core::LinkDefinition,
        provider_main::{get_host_bridge, load_host_data, provider_main, provider_run},
        Context, Message, MessageDispatch, RpcError, RpcResult, SendOpts,
    };

    //pub use crate::Timestamp;
    pub use async_trait::async_trait;
    pub use wasmbus_macros::Provider;

    #[cfg(feature = "BigInteger")]
    pub use num_bigint::BigInt as BigInteger;

    #[cfg(feature = "BigDecimal")]
    pub use bigdecimal::BigDecimal;
}

/// CapabilityProvider handling of messages from host
/// The HostBridge handles most messages and forwards the remainder to this handler
#[async_trait]
pub trait ProviderHandler: Sync {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    /// This message is idempotent - provider must be able to handle
    /// duplicates
    #[allow(unused_variables)]
    async fn put_link(&self, ld: &LinkDefinition) -> Result<bool, RpcError> {
        Ok(true)
    }

    /// Notify the provider that the link is dropped
    #[allow(unused_variables)]
    async fn delete_link(&self, actor_id: &str) {}

    /// Perform health check. Called at regular intervals by host
    /// Default implementation always returns healthy
    #[allow(unused_variables)]
    async fn health_request(
        &self,
        arg: &HealthCheckRequest,
    ) -> Result<HealthCheckResponse, RpcError> {
        Ok(HealthCheckResponse {
            healthy: true,
            message: None,
        })
    }

    /// Handle system shutdown message
    async fn shutdown(&self) -> Result<(), Infallible> {
        Ok(())
    }
}

/// format of log message sent to main thread for output to logger
pub type LogEntry = (log::Level, String);

/// HostBridge manages the NATS connection to the host,
/// and processes subscriptions for links, health-checks, and rpc messages.
/// Callbacks from HostBridge are implemented by the provider in the [[ProviderHandler]] implementation.
///
#[derive(Clone)]
pub struct HostBridge {
    inner: Arc<HostBridgeInner>,
    host_data: HostData,
}

impl HostBridge {
    pub fn new(nats: Arc<NatsClient>, host_data: &HostData) -> Result<HostBridge, RpcError> {
        let key = if host_data.is_test() {
            wascap::prelude::KeyPair::new_user()
        } else {
            wascap::prelude::KeyPair::from_seed(&host_data.invocation_seed)
                .map_err(|e| RpcError::NotInitialized(format!("key failure: {}", e)))?
        };
        let rpc_client = crate::rpc_client::RpcClient::new(
            nats,
            &host_data.lattice_rpc_prefix,
            key,
            host_data.host_id.clone(),
        );

        Ok(HostBridge {
            inner: Arc::new(HostBridgeInner {
                subs: RwLock::new(Vec::new()),
                links: RwLock::new(HashMap::new()),
                rpc_client,
                lattice_prefix: host_data.lattice_rpc_prefix.clone(),
            }),
            host_data: host_data.clone(),
        })
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
    subs: RwLock<Vec<SubscriptionId>>,
    /// Table of actors that are bound to this provider
    /// Key is actor_id / actor public key
    links: RwLock<HashMap<String, LinkDefinition>>,
    rpc_client: crate::rpc_client::RpcClient,
    lattice_prefix: String,
}

impl HostBridge {
    /// Returns a reference to the rpc client
    fn rpc_client(&self) -> &crate::rpc_client::RpcClient {
        &self.rpc_client
    }

    /// Sends an rpc message to the actor.
    pub async fn send_actor(
        &self,
        origin: WasmCloudEntity,
        actor_id: &str,
        message: Message<'_>,
    ) -> Result<Vec<u8>, RpcError> {
        debug!("host_bridge sending actor {}", message.method);
        let target = WasmCloudEntity::new_actor(actor_id)?;
        self.rpc_client().send(origin, target, message).await
    }

    /// Clear out all subscriptions
    async fn unsubscribe_all(&self) {
        let mut copy = Vec::new();
        {
            let mut sub_lock = self.subs.write().await;
            copy.append(&mut sub_lock);
        };
        // async nats client
        let nc = self.rpc_client().get_async().unwrap();
        // unsubscribe each with server and de-register streams
        for sid in copy.iter() {
            // ignore return code; we're shutting down
            if let Err(e) = nc.un_subscribe(sid).await {
                debug!("during shutdown, failure to unsubscribe: {}", e.to_string());
            }
        }
    }

    // add subscription so we can unsubscribe_all later
    async fn add_subscription(&self, sid: SubscriptionId) {
        let mut sub_lock = self.subs.write().await;
        sub_lock.push(sid);
    }

    // parse incoming subscription message
    // if it fails deserialization, we can't really respond;
    // so log the error
    fn parse_msg<T: DeserializeOwned>(&self, msg: &ratsio::NatsMessage, topic: &str) -> Option<T> {
        match if self.host_data.is_test() {
            serde_json::from_slice(&msg.payload).map_err(|e| RpcError::Deser(e.to_string()))
        } else {
            crate::deserialize(&msg.payload)
        } {
            Ok(item) => Some(item),
            Err(e) => {
                error!("garbled data received for {}: {}", topic, e.to_string());
                None
            }
        }
    }

    /// Stores actor with link definition
    pub async fn put_link(&self, ld: LinkDefinition) {
        let mut update = self.links.write().await;
        update.insert(ld.actor_id.to_string(), ld);
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

    /// Returns copy of LinkDefinition, or None,if the actor is not linked
    pub async fn get_link(&self, actor_id: &str) -> Option<LinkDefinition> {
        let read = self.links.read().await;
        read.get(actor_id).cloned()
    }

    /// Implement subscriber listener threads and provider callbacks
    pub async fn connect<P>(
        &'static self,
        provider: P,
        shutdown_tx: oneshot::Sender<HostShutdownEvent>,
    ) -> Result<JoinAll<tokio::task::JoinHandle<Result<(), RpcError>>>, RpcError>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        let join = futures::future::join_all(vec![
            tokio::task::spawn(self.subscribe_rpc(provider.clone())),
            tokio::task::spawn(self.subscribe_link_put(provider.clone())),
            tokio::task::spawn(self.subscribe_link_del(provider.clone())),
            tokio::task::spawn(self.subscribe_shutdown(provider.clone(), shutdown_tx)),
            tokio::task::spawn(self.subscribe_health(provider)),
        ]);
        Ok(join)
    }

    async fn subscribe_rpc<P>(&self, provider: P) -> Result<(), RpcError>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        let rpc_topic = format!(
            "wasmbus.rpc.{}.{}.{}",
            &self.lattice_prefix, &self.host_data.provider_key, self.host_data.link_name
        );

        debug!("subscribing for rpc : {}", &rpc_topic);
        let (sid, sub) = self
            .rpc_client()
            .get_async()
            .unwrap() // we are only async
            .subscribe_with_group(rpc_topic.as_str(), RPC_SUBSCRIPTION_QUEUE_GROUP)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        self.add_subscription(sid).await;
        pin_mut!(sub);
        let provider = provider.clone();
        while let Some(msg) = sub.next().await {
            // parse and validate incoming message
            let response = match crate::deserialize::<Invocation>(&msg.payload) {
                Ok(inv) => match self.validate_invocation(&inv).await {
                    Ok(()) => {
                        trace!(
                            "RPC Invocation: op:{} from:{}",
                            &inv.operation,
                            &inv.origin.public_key
                        );
                        match provider
                            .dispatch(
                                &crate::Context {
                                    actor: Some(inv.origin.public_key.clone()),
                                    ..Default::default()
                                },
                                Message {
                                    method: &inv.operation,
                                    arg: Cow::from(inv.msg),
                                },
                            )
                            .await
                        {
                            Ok(msg) => InvocationResponse {
                                invocation_id: inv.id,
                                error: None,
                                msg: msg.arg.to_vec(),
                            },
                            Err(e) => {
                                error!(
                                    "RPC Invocation failed: op:{} from:{}: {}",
                                    &inv.operation, &inv.origin.public_key, e
                                );
                                InvocationResponse {
                                    invocation_id: inv.id,
                                    error: Some(e.to_string()),
                                    msg: Vec::new(),
                                }
                            }
                        }
                    }
                    Err(s) => {
                        error!(
                            "Invocation validation failure: op:{} from:{} id:{} host:{}: {}",
                            &inv.operation, &inv.origin.public_key, &inv.id, &inv.host_id, &s
                        );
                        InvocationResponse {
                            invocation_id: inv.id,
                            error: Some(s),
                            msg: Vec::new(),
                        }
                    }
                },
                Err(e) => {
                    error!("Invocation deserialization failure: {}", e.to_string());
                    InvocationResponse {
                        invocation_id: "invalid".to_string(),
                        error: Some(format!("Corrupt invocation: {}", e.to_string())),
                        msg: Vec::new(),
                    }
                }
            };
            // return response
            if let Some(reply_to) = msg.reply_to {
                match crate::serialize(&response) {
                    Ok(t) => {
                        if let Err(e) = self.rpc_client().publish(&reply_to, &t).await {
                            error!(
                                "failed sending rpc response to {}: {}",
                                &reply_to,
                                e.to_string()
                            );
                        }
                    }
                    Err(e) => {
                        // extremely unlikely that InvocationResponse would fail to serialize
                        error!("failed serializing InvocationResponse: {}", e.to_string());
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn validate_invocation(&self, inv: &Invocation) -> Result<(), String> {
        let vr = wascap::jwt::validate_token::<wascap::prelude::Invocation>(&inv.encoded_claims)
            .map_err(|e| format!("{}", e))?;
        if vr.expired {
            return Err("Invocation claims token expired".into());
        }
        if !vr.signature_valid {
            return Err("Invocation claims signature invalid".into());
        }
        if vr.cannot_use_yet {
            return Err("Attempt to use invocation before claims token allows".into());
        }
        let target_url = format!("{}/{}", inv.target.url(), &inv.operation);
        let hash = crate::rpc_client::invocation_hash(
            &target_url,
            &inv.origin.url(),
            &inv.operation,
            &inv.msg,
        );
        let claims =
            wascap::prelude::Claims::<wascap::prelude::Invocation>::decode(&inv.encoded_claims)
                .map_err(|e| format!("{}", e))?;
        let inv_claims = claims
            .metadata
            .ok_or_else(|| "No wascap metadata found on claims".to_string())?;
        if inv_claims.invocation_hash != hash {
            return Err(format!(
                "Invocation hash does not match signed claims hash ({} / {})",
                inv_claims.invocation_hash, hash
            ));
        }
        if !inv.host_id.starts_with('N') && inv.host_id.len() != 56 {
            return Err(format!("Invalid host ID on invocation: '{}'", inv.host_id));
        }
        if inv_claims.target_url != target_url {
            return Err(format!(
                "Invocation claims and invocation target URL do not match: {} != {}",
                &inv_claims.target_url, &target_url
            ));
        }
        if inv_claims.origin_url != inv.origin.url() {
            return Err("Invocation claims and invocation origin URL do not match".into());
        }
        // verify target public key is my key
        if inv.target.public_key != self.host_data.provider_key {
            return Err(format!(
                "target key mismatch: {} != {}",
                &inv.target.public_key, &self.host_data.host_id
            ));
        }
        // verify that the sending actor is linked with this provider
        if !self.is_linked(&inv.origin.public_key).await {
            return Err(format!("unlinked actor: {}", &inv.origin.public_key));
        }
        Ok(())
    }

    async fn subscribe_shutdown<P>(
        &self,
        provider: P,
        shutdown_tx: oneshot::Sender<HostShutdownEvent>,
    ) -> Result<(), RpcError>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        let shutdown_topic = format!(
            "wasmbus.rpc.{}.{}.{}.shutdown",
            &self.lattice_prefix, &self.host_data.provider_key, self.host_data.link_name
        );
        debug!("subscribing for shutdown : {}", &shutdown_topic);
        let (subscription_sid, mut sub) = self
            .rpc_client()
            .get_async()
            .unwrap() // we are only async
            .subscribe(&shutdown_topic)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        let (this, provider) = (self.clone(), provider.clone());

        // this while loop doesn't actually loop - it accepts the first signal received.
        // leaving it as a loop for now because it needs to check
        // validate the host signature, and loop if validation fails.
        #[allow(clippy::never_loop)]
        while let Some(msg) = sub.next().await {
            // TODO: verify host's signature before quitting
            // (not sure if host is signing these syet)
            // if not a valid signed message, keep waiting for message
            // if !msg.is_valid() { continue; }

            debug!("Received termination signal. Shutting down capability provider.");
            // tell provider to shutdown - before we shut down nats subscriptions,
            // in case it needs to do any message passing during shutdown
            if let Err(e) = provider.shutdown().await {
                error!("during provider shutdown processing, got error: {}", e);
            }

            // drain all subscriptions except this one
            self.unsubscribe_all().await;

            // send ack to host
            if let Some(reply_to) = msg.reply_to.as_ref() {
                let data = b"shutting down".to_vec();
                if let Err(e) = self.rpc_client().publish(reply_to, &data).await {
                    error!(
                        "failed to send shutdown response to host: {}",
                        e.to_string()
                    );
                }
            }
            break;
        }

        // unsubscribe for shutdowns
        let _ignore = this
            .rpc_client()
            .get_async()
            .unwrap() // we are only async
            .un_subscribe(&subscription_sid)
            .await;

        // signal main thread to quit
        if let Err(e) = shutdown_tx.send("bye".to_string()) {
            error!("Problem shutting down:  failure to send signal: {}", e);
        }
        Ok(())
    }

    async fn subscribe_link_put<P>(&self, provider: P) -> Result<(), RpcError>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        let ldput_topic = format!(
            "wasmbus.rpc.{}.{}.{}.linkdefs.put",
            &self.lattice_prefix, &self.host_data.provider_key, &self.host_data.link_name
        );

        debug!("subscribing for link put : {}", &ldput_topic);
        let (sid, mut sub) = self
            .rpc_client()
            .get_async()
            .unwrap() // we are only async
            .subscribe(&ldput_topic)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        self.add_subscription(sid).await;
        let (this, provider) = (self.clone(), provider.clone());
        while let Some(msg) = sub.next().await {
            if let Some(ld) = this.parse_msg::<LinkDefinition>(&msg, "link.put") {
                if this.is_linked(&ld.actor_id).await {
                    warn!(
                        "Ignoring duplicate link put for '{}' to '{}'.",
                        &ld.actor_id, &ld.provider_id
                    );
                } else {
                    info!("Linking '{}' with '{}'", &ld.actor_id, &ld.provider_id);
                    match provider.put_link(&ld).await {
                        Ok(true) => {
                            this.put_link(ld).await;
                        }
                        Ok(false) => {
                            // authorization failed or parameters were invalid
                            warn!("put_link denied: {}", &ld.actor_id);
                        }
                        Err(e) => {
                            error!("put_link {} failed: {}", &ld.actor_id, e.to_string());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn subscribe_link_del<P>(&self, provider: P) -> Result<(), RpcError>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        // Link Delete
        let link_del_topic = format!(
            "wasmbus.rpc.{}.{}.{}.linkdefs.del",
            &self.lattice_prefix, &self.host_data.provider_key, &self.host_data.link_name
        );
        debug!("subscribing for link del : {}", &link_del_topic);
        let (sid, mut sub) = self
            .rpc_client()
            .get_async()
            .unwrap() // we are only async
            .subscribe(&link_del_topic)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        self.add_subscription(sid).await;
        //let (this, provider) = (self.clone(), provider_impl.clone());
        while let Some(msg) = sub.next().await {
            let (this, provider) = (self.clone(), provider.clone());
            if let Some(ld) = &this.parse_msg::<LinkDefinition>(&msg, "link.del") {
                this.delete_link(&ld.actor_id).await;
                // notify provider that link is deleted
                provider.delete_link(&ld.actor_id).await;
            }
        }
        Ok(())
    }

    async fn subscribe_health<P>(&self, provider: P) -> Result<(), RpcError>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        let topic = format!(
            "wasmbus.rpc.{}.{}.{}.health",
            &self.lattice_prefix, &self.host_data.provider_key, &self.host_data.link_name
        );

        let (sid, mut sub) = self
            .rpc_client()
            .get_async()
            .unwrap() // we are only async
            .subscribe(&topic)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        self.add_subscription(sid).await;
        while let Some(msg) = sub.next().await {
            // placeholder arg
            let arg = HealthCheckRequest {};
            let resp = match provider.health_request(&arg).await {
                Ok(resp) => resp,
                Err(e) => {
                    error!("error generating health check response: {}", &e.to_string());
                    HealthCheckResponse {
                        healthy: false,
                        message: Some(e.to_string()),
                    }
                }
            };
            let buf = if self.host_data.is_test() {
                Ok(serde_json::to_vec(&resp).unwrap())
            } else {
                crate::serialize(&resp)
            };
            match buf {
                Ok(t) => {
                    if let Some(reply_to) = msg.reply_to.as_ref() {
                        if let Err(e) = self.rpc_client().publish(reply_to, &t).await {
                            error!("failed sending health check response: {}", e.to_string());
                        }
                    }
                }
                Err(e) => {
                    // extremely unlikely that InvocationResponse would fail to serialize
                    error!("failed serializing HealthCheckResponse: {}", e.to_string());
                }
            }
        }
        Ok(())
    }
}

pub struct ProviderTransport<'send> {
    pub bridge: &'send HostBridge,
    pub ld: &'send LinkDefinition,
    timeout: std::time::Duration,
}

impl<'send> ProviderTransport<'send> {
    /// constructs a ProviderTransport with the LinkDefinition and bridge.
    /// If the bridge parameter is None, the current (static) bridge is used.
    pub fn new(ld: &'send LinkDefinition, bridge: Option<&'send HostBridge>) -> Self {
        Self::new_with_timeout(ld, bridge, None)
    }

    /// constructs a ProviderTransport with the LinkDefinition, bridge,
    /// and an optional rpc timeout.
    /// If the bridge parameter is None, the current (static) bridge is used.
    pub fn new_with_timeout(
        ld: &'send LinkDefinition,
        bridge: Option<&'send HostBridge>,
        timeout: Option<std::time::Duration>,
    ) -> Self {
        let bridge = bridge.unwrap_or_else(|| crate::provider_main::get_host_bridge());
        Self {
            bridge,
            ld,
            timeout: timeout.unwrap_or(crate::rpc_client::DEFAULT_RPC_TIMEOUT_MILLIS),
        }
    }
}

#[async_trait]
impl<'send> crate::Transport for ProviderTransport<'send> {
    async fn send(
        &self,
        _ctx: &crate::Context,
        req: Message<'_>,
        _opts: Option<crate::SendOpts>,
    ) -> std::result::Result<Vec<u8>, RpcError> {
        let origin = self.ld.provider_entity();
        let target = self.ld.actor_entity();
        self.bridge
            .rpc_client()
            .send_timeout(origin, target, req, self.timeout)
            .await
    }
}
