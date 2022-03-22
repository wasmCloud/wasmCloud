#![cfg(not(target_arch = "wasm32"))]

//! common provider wasmbus support
//!
use std::{
    borrow::Cow,
    collections::HashMap,
    convert::Infallible,
    ops::Deref,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use async_trait::async_trait;
use cfg_if::cfg_if;
use futures::future::JoinAll;
use serde::de::DeserializeOwned;
use tokio::sync::{oneshot, RwLock};
use tracing::{debug, error, info, trace, warn};
use tracing_futures::Instrument;

#[cfg(all(feature = "chunkify", not(target_arch = "wasm32")))]
use crate::chunkify::chunkify_endpoint;
pub use crate::rpc_client::make_uuid;
use crate::{
    common::{deserialize, serialize, Context, Message, MessageDispatch, SendOpts, Transport},
    core::{
        HealthCheckRequest, HealthCheckResponse, HostData, Invocation, InvocationResponse,
        LinkDefinition,
    },
    error::{RpcError, RpcResult},
    rpc_client::{NatsClientType, RpcClient, DEFAULT_RPC_TIMEOUT_MILLIS},
};

// name of nats queue group for rpc subscription
const RPC_SUBSCRIPTION_QUEUE_GROUP: &str = "rpc";

/// nats address to use if not included in initial HostData
pub(crate) const DEFAULT_NATS_ADDR: &str = "nats://127.0.0.1:4222";

pub type HostShutdownEvent = String;

pub trait ProviderDispatch: MessageDispatch + ProviderHandler {}
trait ProviderImpl: ProviderDispatch + Send + Sync + Clone + 'static {}

pub mod prelude {
    pub use crate::{
        common::{Context, Message, MessageDispatch, SendOpts},
        core::LinkDefinition,
        error::{RpcError, RpcResult},
        provider::{HostBridge, ProviderDispatch, ProviderHandler},
        provider_main::{
            get_host_bridge, load_host_data, provider_main, provider_run, provider_start,
        },
    };

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
    async fn put_link(&self, ld: &LinkDefinition) -> RpcResult<bool> {
        Ok(true)
    }

    /// Notify the provider that the link is dropped
    #[allow(unused_variables)]
    async fn delete_link(&self, actor_id: &str) {}

    /// Perform health check. Called at regular intervals by host
    /// Default implementation always returns healthy
    #[allow(unused_variables)]
    async fn health_request(&self, arg: &HealthCheckRequest) -> RpcResult<HealthCheckResponse> {
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
pub type LogEntry = (tracing::Level, String);

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
    pub fn new(nats: crate::anats::Connection, host_data: &HostData) -> RpcResult<HostBridge> {
        Self::new_client(NatsClientType::Async(nats), host_data)
    }

    #[cfg(all(feature = "chunkify", not(target_arch = "wasm32")))]
    pub(crate) fn new_sync_client(&self) -> RpcResult<nats::Connection> {
        //let key = if self.host_data.is_test() {
        //    wascap::prelude::KeyPair::new_user()
        //} else {
        //    wascap::prelude::KeyPair::from_seed(&self.host_data.invocation_seed)
        //        .map_err(|e| RpcError::NotInitialized(format!("key failure: {}", e)))?
        //};
        let nats_addr = if !self.host_data.lattice_rpc_url.is_empty() {
            self.host_data.lattice_rpc_url.as_str()
        } else {
            DEFAULT_NATS_ADDR
        };
        let nats_opts = match (
            self.host_data.lattice_rpc_user_jwt.trim(),
            self.host_data.lattice_rpc_user_seed.trim(),
        ) {
            ("", "") => nats::Options::default(),
            (rpc_jwt, rpc_seed) => {
                let kp = nkeys::KeyPair::from_seed(rpc_seed).unwrap();
                let jwt = rpc_jwt.to_owned();
                nats::Options::with_jwt(
                    move || Ok(jwt.to_owned()),
                    move |nonce| kp.sign(nonce).unwrap(),
                )
            }
        };
        // Connect to nats
        nats_opts
            .max_reconnects(None)
            .connect(nats_addr)
            .map_err(|e| {
                RpcError::ProviderInit(format!("nats connection to {} failed: {}", nats_addr, e))
            })
    }

    pub(crate) fn new_client(nats: NatsClientType, host_data: &HostData) -> RpcResult<HostBridge> {
        let key = if host_data.is_test() {
            wascap::prelude::KeyPair::new_user()
        } else {
            wascap::prelude::KeyPair::from_seed(&host_data.invocation_seed)
                .map_err(|e| RpcError::NotInitialized(format!("key failure: {}", e)))?
        };
        let rpc_client = RpcClient::new_client(
            nats,
            &host_data.lattice_rpc_prefix,
            key,
            host_data.host_id.clone(),
            None,
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

    /// Returns the provider's public key
    pub fn provider_key(&self) -> &str {
        self.host_data.provider_key.as_str()
    }

    /// Returns the host id that launched this provider
    pub fn host_id(&self) -> &str {
        self.host_data.host_id.as_str()
    }

    /// Returns the link_name for this provider
    pub fn link_name(&self) -> &str {
        self.host_data.link_name.as_str()
    }
}

impl Deref for HostBridge {
    type Target = HostBridgeInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[doc(hidden)]
/// Initialize host bridge for use by wasmbus-test-util.
/// The purpose is so that test code can get the nats configuration
/// This is never called inside a provider process (and will fail if a provider calls it)
pub fn init_host_bridge_for_test(
    nc: crate::anats::Connection,
    host_data: &HostData,
) -> crate::error::RpcResult<()> {
    let hb = HostBridge::new(nc, host_data)?;
    crate::provider_main::set_host_bridge(hb)
        .map_err(|_| RpcError::Other("HostBridge already initialized".to_string()))?;
    Ok(())
}

#[doc(hidden)]
pub struct HostBridgeInner {
    subs: RwLock<Vec<crate::anats::Subscription>>,
    /// Table of actors that are bound to this provider
    /// Key is actor_id / actor public key
    links: RwLock<HashMap<String, LinkDefinition>>,
    rpc_client: RpcClient,
    lattice_prefix: String,
}

impl HostBridge {
    /// Returns a reference to the rpc client
    fn rpc_client(&self) -> &RpcClient {
        &self.rpc_client
    }

    /// Clear out all subscriptions
    async fn unsubscribe_all(&self) {
        let mut copy = Vec::new();
        {
            let mut sub_lock = self.subs.write().await;
            copy.append(&mut sub_lock);
        };
        // `drop`ping the Subscription doesn't close it - we need to unsubscribe
        for sub in copy.into_iter() {
            if let Err(e) = sub.close().await {
                debug!(error = %e, "failure to unsubscribe during shutdown");
            }
        }
        debug!("unsubscribed from all subscriptions");
    }

    // add subscription so we can unsubscribe_all later
    async fn add_subscription(&self, sub: crate::anats::Subscription) {
        let mut sub_lock = self.subs.write().await;
        sub_lock.push(sub);
    }

    // parse incoming subscription message
    // if it fails deserialization, we can't really respond;
    // so log the error
    fn parse_msg<T: DeserializeOwned>(
        &self,
        msg: &crate::anats::Message,
        topic: &str,
    ) -> Option<T> {
        match if self.host_data.is_test() {
            serde_json::from_slice(&msg.data).map_err(|e| RpcError::Deser(e.to_string()))
        } else {
            deserialize(&msg.data)
        } {
            Ok(item) => Some(item),
            Err(e) => {
                error!(%topic, error = %e, "garbled data received on topic");
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
    ) -> RpcResult<JoinAll<tokio::task::JoinHandle<RpcResult<()>>>>
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

    async fn subscribe_rpc<P>(&self, provider: P) -> RpcResult<()>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        let rpc_topic = format!(
            "wasmbus.rpc.{}.{}.{}",
            &self.lattice_prefix, &self.host_data.provider_key, self.host_data.link_name
        );

        debug!(%rpc_topic, "subscribing for rpc");
        let sub = self
            .rpc_client()
            .get_async()
            .unwrap() // we are only async
            .queue_subscribe(&rpc_topic, RPC_SUBSCRIPTION_QUEUE_GROUP)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        self.add_subscription(sub.clone()).await;
        let this = self.clone();
        tokio::spawn(async move {
            while let Some(msg) = sub.next().await {
                let span = tracing::trace_span!("subscribe_rpc", %rpc_topic);
                let _enter = span.enter();
                match deserialize::<Invocation>(&msg.data) {
                    Ok(mut inv) => {
                        match this.dechunk_validate(&mut inv).in_current_span().await {
                            Ok(()) => {
                                let provider = provider.clone();
                                let rpc_client = this.rpc_client().clone();
                                // NOTE: This should just spawn with its own span as we aren't
                                // awaiting anything here. If for some reason there are some funky
                                // traces, we can try manually exiting the span here
                                tokio::task::spawn(
                                    async move {
                                        tracing::span::Span::current().record(
                                            "operation",
                                            &tracing::field::display(&inv.operation),
                                        );
                                        tracing::span::Span::current().record(
                                            "public_key",
                                            &tracing::field::display(&inv.origin.public_key),
                                        );
                                        trace!("Dispatching RPC invocation");
                                        let response = match provider
                                            .dispatch(
                                                &Context {
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
                                                msg: msg.arg.to_vec(),
                                                ..Default::default()
                                            },
                                            Err(e) => {
                                                error!(
                                                    error = %e,
                                                    "RPC invocation failed",
                                                );
                                                InvocationResponse {
                                                    invocation_id: inv.id,
                                                    error: Some(e.to_string()),
                                                    ..Default::default()
                                                }
                                            }
                                        };
                                        if let Some(reply_to) = msg.reply {
                                            // Errors are published from inside the function, safe to ignore Result
                                            let _ = publish_invocation_response(
                                                &rpc_client,
                                                reply_to,
                                                response,
                                            )
                                            .await;
                                        }
                                    }
                                    .instrument(
                                        tracing::error_span!(
                                            "invocation_dispatch",
                                            operation = tracing::field::Empty,
                                            public_key = tracing::field::Empty
                                        ),
                                    ),
                                );
                            }
                            Err(s) => {
                                error!(
                                    operation = %inv.operation,
                                    public_key = %inv.origin.public_key,
                                    invocation_id = %inv.id,
                                    host_id = %inv.host_id,
                                    error = %s,
                                    "Invocation validation failure"
                                );

                                if let Some(reply_to) = msg.reply {
                                    // Errors are published from inside the function, safe to ignore Result
                                    let _ = publish_invocation_response(
                                        this.rpc_client(),
                                        reply_to,
                                        InvocationResponse {
                                            invocation_id: inv.id,
                                            error: Some(s.to_string()),
                                            ..Default::default()
                                        },
                                    )
                                    .in_current_span()
                                    .await;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "Invocation deserialization failure");
                        if let Some(reply_to) = msg.reply {
                            if let Err(e) = publish_invocation_response(
                                this.rpc_client(),
                                reply_to,
                                InvocationResponse {
                                    invocation_id: "invalid".to_string(),
                                    error: Some(format!("Corrupt invocation: {}", e)),
                                    ..Default::default()
                                },
                            )
                            .in_current_span()
                            .await
                            {
                                error!(error = %e, "error when replying to rpc");
                            }
                        }
                    }
                }
            }
        });
        Ok(())
    }

    async fn dechunk_validate(&self, inv: &mut Invocation) -> RpcResult<()> {
        #[cfg(all(feature = "chunkify", not(target_arch = "wasm32")))]
        if inv.content_length.is_some() && inv.content_length.unwrap() > inv.msg.len() as u64 {
            let inv_id = inv.id.clone();
            let lattice = self.rpc_client.lattice_prefix().to_string();
            inv.msg = tokio::task::spawn_blocking(move || {
                let ce = chunkify_endpoint(None, lattice)
                    .map_err(|e| format!("connecting for de-chunkifying: {}", &e.to_string()))?;
                ce.get_unchunkified(&inv_id).map_err(|e| e.to_string())
            })
            .await
            .map_err(|je| format!("join/dechunk-validate: {}", je))??;
        }
        self.validate_invocation(inv).await.map_err(RpcError::Rpc)?;
        Ok(())
    }

    async fn validate_invocation(&self, inv: &Invocation) -> Result<(), String> {
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
        if !self.host_data.cluster_issuers.contains(&claims.issuer) {
            return Err("Issuer of this invocation is not in list of cluster issuers".into());
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
            return Err(format!("unlinked actor: '{}'", &inv.origin.public_key));
        }
        Ok(())
    }

    async fn subscribe_shutdown<P>(
        &self,
        provider: P,
        shutdown_tx: oneshot::Sender<HostShutdownEvent>,
    ) -> RpcResult<()>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        let shutdown_topic = format!(
            "wasmbus.rpc.{}.{}.{}.shutdown",
            &self.lattice_prefix, &self.host_data.provider_key, self.host_data.link_name
        );
        debug!("subscribing for shutdown : {}", &shutdown_topic);
        let sub = self
            .rpc_client()
            .get_async()
            .unwrap() // we are only async
            .subscribe(&shutdown_topic)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        // TODO: there should be validation on this message, but it's not signed by host yet
        let msg = sub.next().await;

        // Shutdown messages are unsigned (see https://github.com/wasmCloud/wasmcloud-otp/issues/256)
        // so we can't verify that this came from a trusted source.
        // When the above issue is fixed, verify the source and keep looping if it's invalid.
        eprintln!("Received termination signal. Shutting down capability provider.");
        debug!("Received termination signal. Shutting down capability provider.");
        let (this, provider) = (self.clone(), provider.clone());
        if let Err(e) = tokio::spawn(async move {
            // Tell provider to shutdown - before we shut down nats subscriptions,
            // in case it needs to do any message passing during shutdown
            if let Err(e) = provider.shutdown().await {
                error!(error = %e, "got error during provider shutdown processing");
            }

            // drain all subscriptions except this one
            this.unsubscribe_all().await;
        })
        .await
        {
            error!(error = %e, "joining thread shutdown/unsubscribe task");
        }
        // send ack to host
        if let Some(crate::anats::Message {
            reply: Some(reply_to),
            ..
        }) = msg.as_ref()
        {
            let data = b"shutting down".to_vec();
            if let Err(e) = self.rpc_client().publish(reply_to, &data).await {
                error!(error = %e, "failed to send shutdown ack");
            }
        }

        // unsubscribe from shutdown messages
        let _ = sub.close().await; // ignore errors

        // signal main thread to quit
        if let Err(e) = shutdown_tx.send("bye".to_string()) {
            error!(error = %e, "Problem shutting down:  failure to send signal");
        }
        Ok(())
    }

    async fn subscribe_link_put<P>(&self, provider: P) -> RpcResult<()>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        let ldput_topic = format!(
            "wasmbus.rpc.{}.{}.{}.linkdefs.put",
            &self.lattice_prefix, &self.host_data.provider_key, &self.host_data.link_name
        );

        debug!("subscribing for link put : {}", &ldput_topic);
        let sub = self
            .rpc_client()
            .get_async()
            .unwrap() // we are only async
            .subscribe(&ldput_topic)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        self.add_subscription(sub.clone()).await;
        //let provider = provider.clone();
        let (this, provider) = (self.clone(), provider.clone());
        tokio::spawn(async move {
            // TODO(ss): do we need to pin it with stream() before iterating?
            while let Some(msg) = sub.next().await {
                let span = tracing::error_span!(
                    "subscribe_link_put",
                    actor_id = tracing::field::Empty,
                    provider_id = tracing::field::Empty
                );
                let _enter = span.enter();
                if let Some(ld) = this.parse_msg::<LinkDefinition>(&msg, "link.put") {
                    span.record("actor_id", &tracing::field::display(&ld.actor_id));
                    span.record("provider_id", &tracing::field::display(&ld.provider_id));
                    if this.is_linked(&ld.actor_id).in_current_span().await {
                        warn!("Ignoring duplicate link put");
                    } else {
                        info!("Linking actor with provider");
                        match provider.put_link(&ld).in_current_span().await {
                            Ok(true) => {
                                this.put_link(ld).in_current_span().await;
                            }
                            Ok(false) => {
                                // authorization failed or parameters were invalid
                                warn!("put_link denied");
                            }
                            Err(e) => {
                                error!(error = %e, "put_link failed");
                            }
                        }
                    }
                }
            }
        });
        Ok(())
    }

    async fn subscribe_link_del<P>(&self, provider: P) -> RpcResult<()>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        // Link Delete
        let link_del_topic = format!(
            "wasmbus.rpc.{}.{}.{}.linkdefs.del",
            &self.lattice_prefix, &self.host_data.provider_key, &self.host_data.link_name
        );
        debug!(topic = %link_del_topic, "subscribing for link del");
        let sub = self
            .rpc_client()
            .get_async()
            .unwrap() // we are only async
            .subscribe(&link_del_topic)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        self.add_subscription(sub.clone()).await;
        let (this, provider) = (self.clone(), provider.clone());
        tokio::spawn(async move {
            while let Some(msg) = sub.next().await {
                let span = tracing::trace_span!("subscribe_link_del", topic = %link_del_topic);
                let _enter = span.enter();
                if let Some(ld) = &this.parse_msg::<LinkDefinition>(&msg, "link.del") {
                    this.delete_link(&ld.actor_id).in_current_span().await;
                    // notify provider that link is deleted
                    provider.delete_link(&ld.actor_id).in_current_span().await;
                }
            }
        });
        Ok(())
    }

    async fn subscribe_health<P>(&self, provider: P) -> RpcResult<()>
    where
        P: ProviderDispatch + Send + Sync + Clone + 'static,
    {
        let topic = format!(
            "wasmbus.rpc.{}.{}.{}.health",
            &self.lattice_prefix, &self.host_data.provider_key, &self.host_data.link_name
        );

        let sub = self
            .rpc_client()
            .get_async()
            .unwrap() // we are only async
            .subscribe(&topic)
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        self.add_subscription(sub.clone()).await;
        let this = self.clone();
        tokio::spawn(async move {
            while let Some(msg) = sub.next().await {
                // placeholder arg
                let arg = HealthCheckRequest {};
                let resp = match provider.health_request(&arg).await {
                    Ok(resp) => resp,
                    Err(e) => {
                        error!(error = %e, "error generating health check response");
                        HealthCheckResponse {
                            healthy: false,
                            message: Some(e.to_string()),
                        }
                    }
                };
                let buf = if this.host_data.is_test() {
                    Ok(serde_json::to_vec(&resp).unwrap())
                } else {
                    serialize(&resp)
                };
                match buf {
                    Ok(t) => {
                        if let Some(reply_to) = msg.reply.as_ref() {
                            if let Err(e) = this.rpc_client().publish(reply_to, &t).await {
                                error!(error = %e, "failed sending health check response");
                            }
                        }
                    }
                    Err(e) => {
                        // extremely unlikely that InvocationResponse would fail to serialize
                        error!(error = %e, "failed serializing HealthCheckResponse");
                    }
                }
            }
        });
        Ok(())
    }
}

async fn publish_invocation_response(
    rpc_client: &RpcClient,
    reply_to: String,
    response: InvocationResponse,
) -> Result<(), String> {
    let content_length = Some(response.msg.len() as u64);

    let response = {
        cfg_if! {
            if #[cfg(all(feature = "chunkify", not(target_arch = "wasm32")))] {
                let inv_id = response.invocation_id.clone();
                if crate::chunkify::needs_chunking(response.msg.len()) {
                    let msg = response.msg;
                    let lattice = rpc_client.lattice_prefix().to_string();
            tokio::task::spawn_blocking(move || {
                let ce = chunkify_endpoint(None, lattice)
                    .map_err(|e| format!("connecting for chunkifying: {}", &e.to_string()))?;
                ce.chunkify_response(&inv_id, &mut msg.as_slice())
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|je| format!("join/response-chunk: {}", je))??;
            InvocationResponse {
                msg: Vec::new(),
                content_length,
                ..response
            }
        } else {
            InvocationResponse {
                content_length,
                ..response
            }
        }

                } else {
                    InvocationResponse {
                        content_length,
                        ..response
                    }
                }
            }
    };

    match serialize(&response) {
        Ok(t) => {
            if let Err(e) = rpc_client.publish(&reply_to, &t).await {
                error!(
                    %reply_to,
                    error = %e,
                    "failed sending rpc response",
                );
            }
        }
        Err(e) => {
            // extremely unlikely that InvocationResponse would fail to serialize
            error!(error = %e, "failed serializing InvocationResponse");
        }
    }
    Ok(())
}

pub struct ProviderTransport<'send> {
    pub bridge: &'send HostBridge,
    pub ld: &'send LinkDefinition,
    timeout: StdMutex<std::time::Duration>,
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
        #[allow(clippy::redundant_closure)]
        let bridge = bridge.unwrap_or_else(|| crate::provider_main::get_host_bridge());
        Self {
            bridge,
            ld,
            timeout: StdMutex::new(timeout.unwrap_or_else(|| {
                bridge
                    .host_data
                    .default_rpc_timeout_ms
                    .map(|t| Duration::from_millis(t as u64))
                    .unwrap_or(DEFAULT_RPC_TIMEOUT_MILLIS)
            })),
        }
    }
}

#[async_trait]
impl<'send> Transport for ProviderTransport<'send> {
    async fn send(
        &self,
        _ctx: &Context,
        req: Message<'_>,
        _opts: Option<SendOpts>,
    ) -> RpcResult<Vec<u8>> {
        let origin = self.ld.provider_entity();
        let target = self.ld.actor_entity();
        let timeout = {
            if let Ok(rd) = self.timeout.lock() {
                *rd
            } else {
                // if lock is poisioned
                warn!("rpc timeout mutex error - using default value");
                self.bridge
                    .host_data
                    .default_rpc_timeout_ms
                    .map(|t| Duration::from_millis(t as u64))
                    .unwrap_or(DEFAULT_RPC_TIMEOUT_MILLIS)
            }
        };
        self.bridge
            .rpc_client()
            .send_timeout(origin, target, req, timeout)
            .await
    }

    fn set_timeout(&self, interval: Duration) {
        if let Ok(mut write) = self.timeout.lock() {
            *write = interval;
        } else {
            warn!("rpc timeout mutex error - unchanged")
        }
    }
}
