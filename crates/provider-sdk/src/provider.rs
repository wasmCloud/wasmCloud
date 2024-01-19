use std::{borrow::Cow, collections::HashMap, fmt::Formatter, sync::Arc, time::Duration};

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{Mutex, RwLock},
    task::JoinHandle,
};
use tracing::{debug, error, info, instrument, trace, warn};
use tracing_futures::Instrument;
use wascap::{
    jwt,
    prelude::{Claims, KeyPair},
};

use wasmcloud_core::{
    HealthCheckRequest, HostData, Invocation, InvocationResponse, LinkDefinition,
};
#[cfg(feature = "otel")]
use wasmcloud_tracing::context::attach_span_context;

use crate::{
    deserialize,
    error::{
        InvocationError, InvocationResult, ProviderInitError, ProviderInitResult, ValidationError,
    },
    rpc_client::RpcClient,
    serialize, Context, Provider,
};

// name of nats queue group for rpc subscription
const RPC_SUBSCRIPTION_QUEUE_GROUP: &str = "rpc";

pub type QuitSignal = tokio::sync::broadcast::Receiver<bool>;

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
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = $channel.recv() => {
                        let _ = $sub.unsubscribe().await;
                        break;
                    },
                    __msg = $sub.next() => {
                        match __msg {
                            None => break,
                            Some($msg) => {
                                $on_item
                            }
                        }
                    }
                }
            }
        })
    };
}

#[derive(Clone)]
pub struct ProviderConnection {
    links: Arc<RwLock<HashMap<String, LinkDefinition>>>,
    rpc_client: RpcClient,
    lattice: String,
    host_data: Arc<HostData>,
    // We keep these around so they can drop
    _listener_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl std::fmt::Debug for ProviderConnection {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConnection")
            .field("provider_id", &self.host_data.provider_key)
            .field("host_id", &self.host_data.host_id)
            .field("link", &self.host_data.link_name)
            .field("lattice", &self.lattice)
            .finish()
    }
}

impl ProviderConnection {
    pub(crate) fn new(
        nats: async_nats::Client,
        host_data: &HostData,
    ) -> ProviderInitResult<ProviderConnection> {
        let key = Arc::new(
            KeyPair::from_seed(&host_data.invocation_seed)
                .map_err(|e| ProviderInitError::Initialization(format!("key failure: {e}")))?,
        );

        let rpc_client = RpcClient::new(
            nats,
            host_data.host_id.clone(),
            host_data.default_rpc_timeout_ms.map(Duration::from_millis),
            key,
            &host_data.lattice_rpc_prefix,
        );

        Ok(ProviderConnection {
            links: Arc::new(RwLock::new(HashMap::new())),
            rpc_client,
            lattice: host_data.lattice_rpc_prefix.to_owned(),
            host_data: Arc::new(host_data.to_owned()),
            _listener_handles: Default::default(),
        })
    }

    /// Used for fetching the RPC client in order to make RPC calls
    pub fn get_rpc_client(&self) -> RpcClient {
        self.rpc_client.clone()
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

    /// Implement subscriber listener threads and provider callbacks
    pub(crate) async fn connect<P>(
        &self,
        provider: P,
        shutdown_tx: &tokio::sync::broadcast::Sender<bool>,
        lattice: &str,
    ) -> ProviderInitResult<()>
    where
        P: Provider + Clone,
    {
        let lattice = lattice.to_string();
        let mut handles = Vec::new();
        handles.push(
            self.subscribe_rpc(provider.clone(), shutdown_tx.subscribe(), lattice)
                .await?,
        );
        handles.push(
            self.subscribe_link_put(provider.clone(), shutdown_tx.subscribe())
                .await?,
        );
        handles.push(
            self.subscribe_link_del(provider.clone(), shutdown_tx.subscribe())
                .await?,
        );
        handles.push(
            self.subscribe_shutdown(provider.clone(), shutdown_tx.clone())
                .await?,
        );
        handles.push(
            self.subscribe_health(provider, shutdown_tx.subscribe())
                .await?,
        );
        let mut lock = self._listener_handles.lock().await;
        *lock = handles;
        Ok(())
    }

    /// flush nats - called before main process exits
    pub(crate) async fn flush(&self) {
        self.rpc_client.flush().await
    }

    /// Returns the nats rpc topic for capability providers
    pub fn provider_rpc_topic(&self) -> String {
        format!(
            "wasmbus.rpc.{}.{}.{}",
            &self.lattice, &self.host_data.provider_key, self.host_data.link_name
        )
    }

    /// Subscribe to a nats topic for rpc messages.
    /// This method starts a separate async task and returns immediately.
    /// It will exit if the nats client disconnects, or if a signal is received on the quit channel.
    pub async fn subscribe_rpc<P>(
        &self,
        provider: P,
        mut quit: QuitSignal,
        lattice: String,
    ) -> ProviderInitResult<JoinHandle<()>>
    where
        P: Provider + Clone,
    {
        let mut sub = self
            .rpc_client
            .client()
            .queue_subscribe(
                self.provider_rpc_topic(),
                RPC_SUBSCRIPTION_QUEUE_GROUP.to_string(),
            )
            .await?;
        let this = self.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = quit.recv() => {
                        let _ = sub.unsubscribe().await;
                        break;
                    },
                    nats_msg = sub.next() => {
                        let msg = if let Some(msg) = nats_msg { msg } else { break; };
                        let this = this.clone();
                        let provider = provider.clone();
                        let lattice = lattice.clone();
                        let span = tracing::debug_span!("rpc",
                            operation = tracing::field::Empty,
                            lattice_id = tracing::field::Empty,
                            actor_id = tracing::field::Empty,
                            inv_id = tracing::field::Empty,
                            host_id = tracing::field::Empty,
                            provider_id = tracing::field::Empty,
                            contract_id = tracing::field::Empty,
                            link_name = tracing::field::Empty,
                            payload_size = tracing::field::Empty
                        );
                        tokio::spawn( async move {
                            match deserialize::<Invocation>(&msg.payload) {
                                Ok(inv) => {
                                    #[cfg(feature = "otel")]
                                    if !inv.trace_context.is_empty() {
                                        attach_span_context(&inv.trace_context);
                                    }
                                    let current = tracing::Span::current();
                                    current.record("operation", &tracing::field::display(&inv.operation));
                                    current.record("lattice_id", &tracing::field::display(&lattice));
                                    current.record("actor_id", &tracing::field::display(&inv.origin.public_key));
                                    current.record("inv_id", &tracing::field::display(&inv.id));
                                    current.record("host_id", &tracing::field::display(&inv.host_id));
                                    current.record("provider_id", &tracing::field::display(&inv.target.public_key));
                                    current.record("contract_id", &tracing::field::display(&inv.target.contract_id));
                                    current.record("link_name", &tracing::field::display(&inv.target.link_name));
                                    current.record("payload_size", &tracing::field::display(&inv.content_length));
                                    let inv_id = inv.id.clone();
                                    let inv_operation = inv.operation.clone();
                                    let resp = match this.handle_rpc(provider.clone(), inv).in_current_span().await {
                                        Err(err) => {
                                            error!(%err, operation = %inv_operation, "Invocation failed");
                                            InvocationResponse{
                                                invocation_id: inv_id,
                                                error: Some(format!("Error when handling invocation: {err}")),
                                                ..Default::default()
                                            }
                                        },
                                        Ok(bytes) => {
                                            InvocationResponse{
                                                invocation_id: inv_id,
                                                content_length: bytes.len() as u64,
                                                msg: bytes,
                                                ..Default::default()
                                            }
                                        }
                                    };
                                    if let Some(reply) = msg.reply {
                                        // send reply
                                        if let Err(err) = this.rpc_client
                                            .publish_invocation_response(reply, resp).in_current_span().await {
                                            error!(%err, "rpc sending response");
                                        }
                                    }
                                },
                                Err(err) => {
                                    error!(%err, "invalid rpc message received (not deserializable)");
                                    if let Some(reply) = msg.reply {
                                        if let Err(err) = this.rpc_client.publish_invocation_response(reply,
                                            InvocationResponse{
                                                error: Some(format!("Error when attempting to deserialize invocation: {err}")),
                                                ..Default::default()
                                            },
                                        ).in_current_span().await {
                                            error!(%err, "unable to publish invocation response error");
                                        }
                                    }
                                }
                            };
                        }.instrument(span)); /* spawn */
                    } /* next */
                }
            } /* loop */
        });
        Ok(handle)
    }

    async fn handle_rpc<P>(&self, provider: P, inv: Invocation) -> InvocationResult<Vec<u8>>
    where
        P: Provider + Clone,
    {
        let inv = self.rpc_client.dechunk(inv).await?;
        let (inv, claims) = self
            .rpc_client
            .validate_invocation(inv)
            .await
            .map_err(InvocationError::from)?;
        self.validate_provider_invocation(&inv, &claims)
            .await
            .map_err(InvocationError::from)?;
        let span = tracing::debug_span!("dispatch", public_key = %inv.origin.public_key, method = %inv.operation);
        provider
            .dispatch(
                Context {
                    actor: Some(inv.origin.public_key.clone()),
                    tracing: inv.trace_context.into_iter().collect(),
                },
                inv.operation,
                Cow::Owned(inv.msg),
            )
            .instrument(span)
            .await
    }

    async fn subscribe_shutdown<P>(
        &self,
        provider: P,
        shutdown_tx: tokio::sync::broadcast::Sender<bool>,
    ) -> ProviderInitResult<JoinHandle<()>>
    where
        P: Provider,
    {
        let shutdown_topic = format!(
            "wasmbus.rpc.{}.{}.{}.shutdown",
            &self.lattice, &self.host_data.provider_key, self.host_data.link_name
        );
        debug!("subscribing for shutdown : {}", &shutdown_topic);
        let mut sub = self.rpc_client.client().subscribe(shutdown_topic).await?;
        let rpc_client = self.rpc_client.clone();
        let host_id = self.host_data.host_id.clone();
        let handle = tokio::spawn(
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
                        let shutmsg: ShutdownMessage =
                            serde_json::from_slice(&payload).unwrap_or_default();
                        if shutmsg.host_id == host_id {
                            info!("Received termination signal and stopping");
                            // Tell provider to shutdown - before we shut down nats subscriptions,
                            // in case it needs to do any message passing during shutdown
                            provider.shutdown().await;
                            let data = b"shutting down".to_vec();
                            if let Err(err) = rpc_client.publish(reply_to, data).await {
                                warn!(%err, "failed to send shutdown ack");
                            }
                            // unsubscribe from shutdown topic
                            if let Err(err) = sub.unsubscribe().await {
                                warn!(%err, "failed to unsubscribe from shutdown topic")
                            }
                            // send shutdown signal to all listeners: quit all subscribers and signal main thread to quit
                            if let Err(err) = shutdown_tx.send(true) {
                                error!(%err, "Problem shutting down:  failure to send signal");
                            }
                            break;
                        } else {
                            trace!(
                                "Ignoring termination signal (request targeted for different host)"
                            );
                        }
                    }
                }
            }
            .instrument(tracing::debug_span!("shutdown_subscriber")),
        );

        Ok(handle)
    }

    async fn subscribe_link_put<P>(
        &self,
        provider: P,
        mut quit: QuitSignal,
    ) -> ProviderInitResult<JoinHandle<()>>
    where
        P: Provider + Clone,
    {
        let ldput_topic = format!(
            "wasmbus.rpc.{}.{}.{}.linkdefs.put",
            &self.lattice, &self.host_data.provider_key, &self.host_data.link_name
        );

        let mut sub = self.rpc_client.client().subscribe(ldput_topic).await?;
        let (this, provider) = (self.clone(), provider.clone());
        let handle = tokio::spawn(async move {
            process_until_quit!(sub, quit, msg, {
                this.handle_link_put(msg, &provider).await
            });
        });
        Ok(handle)
    }

    #[instrument(level = "debug", skip_all, fields(actor_id = tracing::field::Empty, provider_id = tracing::field::Empty, contract_id = tracing::field::Empty, link_name = tracing::field::Empty))]
    async fn handle_link_put<P>(&self, msg: async_nats::Message, provider: &P)
    where
        P: Provider,
    {
        match deserialize::<LinkDefinition>(&msg.payload) {
            Ok(ld) => {
                let span = tracing::Span::current();
                span.record("actor_id", &tracing::field::display(&ld.actor_id));
                span.record("provider_id", &tracing::field::display(&ld.provider_id));
                span.record("contract_id", &tracing::field::display(&ld.contract_id));
                span.record("link_name", &tracing::field::display(&ld.link_name));
                if self.is_linked(&ld.actor_id).await {
                    warn!("Ignoring duplicate link put");
                } else {
                    info!("Linking actor with provider");
                    if provider.put_link(&ld).await {
                        self.put_link(ld).await;
                    } else {
                        warn!("put_link denied");
                    }
                }
            }
            Err(err) => {
                error!(%err, "received invalid link def data on message");
            }
        }
    }

    async fn subscribe_link_del<P>(
        &self,
        provider: P,
        mut quit: QuitSignal,
    ) -> ProviderInitResult<JoinHandle<()>>
    where
        P: Provider + Clone,
    {
        // Link Delete
        let link_del_topic = format!(
            "wasmbus.rpc.{}.{}.{}.linkdefs.del",
            &self.lattice, &self.host_data.provider_key, &self.host_data.link_name
        );
        debug!(topic = %link_del_topic, "subscribing for link del");
        let mut sub = self
            .rpc_client
            .client()
            .subscribe(link_del_topic.clone())
            .await?;
        let (this, provider) = (self.clone(), provider.clone());
        let handle = tokio::spawn(async move {
            process_until_quit!(sub, quit, msg, {
                let span = tracing::trace_span!("subscribe_link_del", topic = %link_del_topic);
                if let Ok(ld) = deserialize::<LinkDefinition>(&msg.payload) {
                    this.delete_link(&ld.actor_id)
                        .instrument(span.clone())
                        .await;
                    // notify provider that link is deleted
                    provider.delete_link(&ld.actor_id).instrument(span).await;
                }
            });
        });

        Ok(handle)
    }

    async fn subscribe_health<P>(
        &self,
        provider: P,
        mut quit: QuitSignal,
    ) -> ProviderInitResult<JoinHandle<()>>
    where
        P: Provider,
    {
        let topic = format!(
            "wasmbus.rpc.{}.{}.{}.health",
            &self.lattice, &self.host_data.provider_key, &self.host_data.link_name
        );

        let mut sub = self.rpc_client.client().subscribe(topic).await?;
        let this = self.clone();
        let handle = tokio::spawn(
            async move {
                process_until_quit!(sub, quit, msg, {
                    let resp = provider.health_request(&HealthCheckRequest {}).await;
                    let buf = serialize(&resp);
                    match buf {
                        Ok(t) => {
                            if let Some(reply_to) = msg.reply {
                                if let Err(err) = this.rpc_client.publish(reply_to, t).await {
                                    error!(%err, "failed sending health check response");
                                }
                            }
                        }
                        Err(err) => {
                            // extremely unlikely that InvocationResponse would fail to serialize
                            error!(%err, "failed serializing HealthCheckResponse");
                        }
                    }
                });
            }
            .instrument(tracing::debug_span!("subscribe_health")),
        );

        Ok(handle)
    }

    /// extra validation performed by providers
    async fn validate_provider_invocation(
        &self,
        inv: &Invocation,
        claims: &Claims<jwt::Invocation>,
    ) -> Result<(), ValidationError> {
        if !self.host_data.cluster_issuers.contains(&claims.issuer) {
            return Err(ValidationError::InvalidIssuer);
        }

        // verify target public key is my key
        if inv.target.public_key != self.host_data.provider_key {
            return Err(ValidationError::InvalidTarget(
                inv.target.public_key.clone(),
                self.host_data.provider_key.clone(),
            ));
        }

        // verify that the sending actor is linked with this provider
        if !self.is_linked(&inv.origin.public_key).await {
            return Err(ValidationError::InvalidActor(inv.origin.public_key.clone()));
        }

        Ok(())
    }
}
