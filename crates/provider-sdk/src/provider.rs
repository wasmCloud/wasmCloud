use core::fmt::Formatter;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_nats::HeaderMap;
use futures::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, instrument, trace, warn};
use tracing_futures::Instrument;
use ulid::Ulid;
use uuid::Uuid;
use wasmcloud_core::nats::convert_header_map_to_hashmap;
use wasmcloud_core::wrpc::Client as WrpcNatsClient;
use wasmcloud_core::HealthCheckRequest;
use wasmcloud_core::InterfaceLinkDefinition;
use wrpc_transport::{AcceptedInvocation, Client, Transmitter};

#[cfg(feature = "otel")]
use wasmcloud_core::TraceContext;
#[cfg(feature = "otel")]
use wasmcloud_tracing::context::attach_span_context;

use wrpc_types::DynamicFunction;

use crate::{
    deserialize,
    error::{InvocationResult, ProviderInitError, ProviderInitResult},
    serialize, Context, Provider, WrpcInvocationLookup,
};

/// Name of the header that should be passed for invocations that identifies the source
const WRPC_SOURCE_ID_HEADER_NAME: &str = "source-id";

/// Name of the header that should be passed for invocations that identifies the host from which invocation was run
const WRPC_HEADER_NAME_HOST_ID: &str = "host-id";

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

    /// Handles for every NATS listener that is created and used by the Provider, kept
    /// around so they can appropriately be `Drop`ed when this `ProviderConnection` is
    _listener_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<Result<()>>>>>,

    /// Mapping of NATS subjects to dynamic function information for incoming invocations
    #[allow(unused)]
    incoming_invocation_fn_map: Arc<WrpcInvocationLookup>,
}

impl std::fmt::Debug for ProviderConnection {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConnection")
            .field("provider_id", &self.provider_key())
            .field("host_id", &self.host_id)
            .field("link", &self.link_name)
            .field("lattice", &self.lattice)
            .finish()
    }
}

impl ProviderConnection {
    pub(crate) fn new(
        nats: async_nats::Client,
        provider_key: String,
        lattice: String,
        host_id: String,
        link_name: String,
        incoming_invocation_fn_map: WrpcInvocationLookup,
    ) -> ProviderInitResult<ProviderConnection> {
        Ok(ProviderConnection {
            links: Arc::new(RwLock::new(HashMap::new())),
            nats: Arc::new(nats),
            lattice,
            host_id,
            link_name,
            provider_key,
            _listener_handles: Default::default(),
            incoming_invocation_fn_map: Arc::new(incoming_invocation_fn_map),
        })
    }

    /// Used for fetching the RPC client in order to make RPC calls
    pub fn get_wrpc_client(&self, target: impl AsRef<str>) -> WrpcNatsClient {
        let target = target.as_ref();
        let mut headers = HeaderMap::new();
        headers.insert("source-id", self.provider_key());
        headers.insert("target-id", target);
        WrpcNatsClient::new(Arc::clone(&self.nats), &self.lattice, target, headers)
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

    /// Implement subscriber listener threads and provider callbacks
    pub(crate) async fn connect<P>(
        &self,
        provider: P,
        shutdown_tx: &broadcast::Sender<()>,
        lattice: &str,
    ) -> ProviderInitResult<()>
    where
        P: Provider + Clone,
    {
        let lattice = lattice.to_string();
        let mut handles = Vec::new();
        handles.extend(
            self.subscribe_rpc(
                provider.clone(),
                shutdown_tx.subscribe(),
                lattice,
                &self.provider_key,
            )
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
        if let Err(err) = self.nats.flush().await {
            error!(%err, "error flushing NATS client");
        }
    }

    /// Subscribe to a nats topic for rpc messages.
    /// This method starts a separate async task and returns immediately.
    /// It will exit if the nats client disconnects, or if a signal is received on the quit channel.
    pub async fn subscribe_rpc<P>(
        &self,
        provider: P,
        quit: QuitSignal,
        lattice: String,
        provider_id: impl AsRef<str>,
    ) -> ProviderInitResult<Vec<JoinHandle<Result<()>>>>
    where
        P: Provider + Clone,
    {
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
            handles.push(tokio::spawn(async move {
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

                            // Build a trace context from incoming headers
                            let context = context.unwrap_or_default();
                            #[cfg(feature = "otel")]
                            {
                                let trace_context: TraceContext = convert_header_map_to_hashmap(&context)
                                    .into_iter()
                                    .collect::<Vec<(String, String)>>();
                                attach_span_context(&trace_context);
                            }

                            // Determine source ID for the invocation
                            let source_id = context.get(WRPC_SOURCE_ID_HEADER_NAME).map(ToString::to_string).unwrap_or_else(|| "<unknown>".into());

                            let current = tracing::Span::current();
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

                            // Perform RPC
                            match this.handle_wrpc(provider.clone(), &operation, source_id, params, context).in_current_span().await {
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
    async fn handle_wrpc<P>(
        &self,
        provider: P,
        operation: impl AsRef<str>,
        source_id: impl AsRef<str>,
        invocation_params: Vec<wrpc_transport::Value>,
        context: HeaderMap,
    ) -> InvocationResult<Vec<u8>>
    where
        P: Provider + Clone,
    {
        let operation = operation.as_ref();
        let source_id = source_id.as_ref();

        // Dispatch the invocation to the provider
        let span = tracing::debug_span!("dispatch", %source_id, %operation);
        provider
            .dispatch_wrpc_dynamic(
                Context {
                    actor: Some(source_id.into()),
                    tracing: convert_header_map_to_hashmap(&context),
                },
                operation.to_string(),
                invocation_params,
            )
            .instrument(span)
            .await
    }

    async fn subscribe_shutdown<P>(
        &self,
        provider: P,
        shutdown_tx: broadcast::Sender<()>,
    ) -> ProviderInitResult<JoinHandle<Result<()>>>
    where
        P: Provider,
    {
        let shutdown_topic = format!(
            "wasmbus.rpc.{}.{}.{}.shutdown",
            &self.lattice, &self.provider_key, self.link_name
        );
        debug!("subscribing for shutdown : {}", &shutdown_topic);
        let mut sub = self.nats.subscribe(shutdown_topic).await?;
        let nats = self.nats.clone();
        let host_id = self.host_id.clone();
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
                            if let Err(err) = nats.publish(reply_to, "shutting down".into()).await {
                                warn!(%err, "failed to send shutdown ack");
                            }
                            // unsubscribe from shutdown topic
                            if let Err(err) = sub.unsubscribe().await {
                                warn!(%err, "failed to unsubscribe from shutdown topic")
                            }
                            // send shutdown signal to all listeners: quit all subscribers and signal main thread to quit
                            if let Err(err) = shutdown_tx.send(()) {
                                error!(%err, "Problem shutting down:  failure to send signal");
                            }
                            break;
                        }
                        trace!("Ignoring termination signal (request targeted for different host)");
                    }
                }
                Ok(())
            }
            .instrument(tracing::debug_span!("shutdown_subscriber")),
        );

        Ok(handle)
    }

    async fn subscribe_link_put<P>(
        &self,
        provider: P,
        mut quit: QuitSignal,
    ) -> ProviderInitResult<JoinHandle<Result<()>>>
    where
        P: Provider + Clone,
    {
        let ldput_topic = format!(
            "wasmbus.rpc.{}.{}.linkdefs.put",
            &self.lattice, &self.provider_key,
        );
        let mut sub = self.nats.subscribe(ldput_topic).await?;
        let (this, provider) = (self.clone(), provider.clone());
        let handle = tokio::spawn(async move {
            process_until_quit!(sub, quit, msg, {
                this.handle_link_put(msg, &provider).await
            });
            Ok(())
        });
        Ok(handle)
    }

    #[instrument(level = "debug", skip_all, fields(actor_id = tracing::field::Empty, provider_id = tracing::field::Empty, contract_id = tracing::field::Empty, link_name = tracing::field::Empty))]
    async fn handle_link_put<P>(&self, msg: async_nats::Message, provider: &P)
    where
        P: Provider,
    {
        match deserialize::<InterfaceLinkDefinition>(&msg.payload) {
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
                // If the link has already been put, return early
                if self.is_linked(&ld.source_id).await {
                    warn!("Ignoring duplicate link put");
                    return;
                }

                info!("Linking actor with provider");
                if provider.put_link(&ld).await {
                    self.put_link(ld).await;
                } else {
                    warn!("put_link failed");
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
    ) -> ProviderInitResult<JoinHandle<Result<()>>>
    where
        P: Provider + Clone,
    {
        let link_del_topic = format!(
            "wasmbus.rpc.{}.{}.linkdefs.del",
            &self.lattice, &self.provider_key
        );
        debug!(topic = %link_del_topic, "subscribing for link del");
        let mut sub = self.nats.subscribe(link_del_topic.clone()).await?;
        let (this, provider) = (self.clone(), provider.clone());
        let handle = tokio::spawn(async move {
            process_until_quit!(sub, quit, msg, {
                let span = tracing::trace_span!("subscribe_link_del", topic = %link_del_topic);
                if let Ok(ld) = deserialize::<InterfaceLinkDefinition>(&msg.payload) {
                    this.delete_link(&ld.source_id)
                        .instrument(span.clone())
                        .await;
                    // notify provider that link is deleted
                    provider.delete_link(&ld.source_id).instrument(span).await;
                }
            });
            Ok(())
        });

        Ok(handle)
    }

    async fn subscribe_health<P>(
        &self,
        provider: P,
        mut quit: QuitSignal,
    ) -> ProviderInitResult<JoinHandle<Result<()>>>
    where
        P: Provider,
    {
        let topic = format!(
            "wasmbus.rpc.{}.{}.health",
            &self.lattice, &self.provider_key,
        );

        let mut sub = self.nats.subscribe(topic).await?;
        let this = self.clone();
        let handle = tokio::spawn(
            async move {
                process_until_quit!(sub, quit, msg, {
                    let resp = provider.health_request(&HealthCheckRequest {}).await;
                    let buf = serialize(&resp);
                    match buf {
                        Ok(t) => {
                            if let Some(reply_to) = msg.reply {
                                if let Err(err) = this.nats.publish(reply_to, t.into()).await {
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
                Ok(())
            }
            .instrument(tracing::debug_span!("subscribe_health")),
        );

        Ok(handle)
    }
}
