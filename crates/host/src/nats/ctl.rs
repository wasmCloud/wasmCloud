//! The NATS implementation of the control interface.

use anyhow::Context as _;
use async_nats::jetstream::kv::Store;
use bytes::Bytes;
use futures::future::Either;
use futures::stream::SelectAll;
use futures::{Stream, StreamExt, TryFutureExt as _};
use serde::Serialize;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::task::JoinSet;
use tracing::{error, instrument, trace, warn};
use wasmcloud_control_interface::CtlResponse;
use wasmcloud_core::CTL_API_VERSION_1;
use wasmcloud_tracing::context::TraceContextInjector;

use crate::wasmbus::injector_to_headers;

use super::store::data_watch;

#[derive(Debug)]
pub(crate) struct Queue {
    all_streams: SelectAll<async_nats::Subscriber>,
}

impl Stream for Queue {
    type Item = async_nats::Message;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.all_streams.poll_next_unpin(cx)
    }
}

impl Queue {
    #[instrument]
    pub(crate) async fn new(
        nats: &async_nats::Client,
        topic_prefix: &str,
        lattice: &str,
        host_id: &str,
        component_auction: bool,
        provider_auction: bool,
    ) -> anyhow::Result<Self> {
        let mut subs = vec![
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.registry.put",
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.host.ping",
            ))),
            Either::Right(nats.queue_subscribe(
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.link.*"),
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.link",),
            )),
            Either::Right(nats.queue_subscribe(
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.claims.get"),
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.claims"),
            )),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.component.*.{host_id}"
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.provider.*.{host_id}"
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.label.*.{host_id}"
            ))),
            Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.host.*.{host_id}"
            ))),
            Either::Right(nats.queue_subscribe(
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.config.>"),
                format!("{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.config"),
            )),
        ];
        if component_auction {
            subs.push(Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.component.auction",
            ))));
        }
        if provider_auction {
            subs.push(Either::Left(nats.subscribe(format!(
                "{topic_prefix}.{CTL_API_VERSION_1}.{lattice}.provider.auction",
            ))));
        }
        let streams = futures::future::join_all(subs)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, async_nats::SubscribeError>>()
            .context("failed to subscribe to queues")?;
        Ok(Self {
            all_streams: futures::stream::select_all(streams),
        })
    }
}

impl crate::wasmbus::Host {
    #[instrument(level = "trace", skip_all, fields(subject = %message.subject))]
    pub(crate) async fn handle_ctl_message(
        self: Arc<Self>,
        message: async_nats::Message,
    ) -> Option<Bytes> {
        // NOTE: if log level is not `trace`, this won't have an effect, since the current span is
        // disabled. In most cases that's fine, since we aren't aware of any control interface
        // requests including a trace context
        opentelemetry_nats::attach_span_context(&message);
        // Skip the topic prefix, the version, and the lattice
        // e.g. `wasmbus.ctl.v1.{prefix}`
        let subject = message.subject;
        let mut parts = subject
            .trim()
            // TODO(brooksmtownsend): topic prefix parsing elsewhere
            // .trim_start_matches(&self.ctl_topic_prefix)
            .trim_start_matches("wasmbus.ctl")
            .trim_start_matches('.')
            .split('.')
            .skip(2);
        trace!(%subject, "handling control interface request");

        // This response is a wrapped Result<Option<Result<Vec<u8>>>> for a good reason.
        // The outer Result is for reporting protocol errors in handling the request, e.g. failing to
        //    deserialize the request payload.
        // The Option is for the case where the request is handled successfully, but the handler
        //    doesn't want to send a response back to the client, like with an auction.
        // The inner Result is purely for the success or failure of serializing the [CtlResponse], which
        //    should never fail but it's a result we must handle.
        // And finally, the Vec<u8> is the serialized [CtlResponse] that we'll send back to the client
        let ctl_response = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            // Component commands
            (Some("component"), Some("auction"), None, None) => self
                .handle_auction_component(message.payload)
                .await
                .map(serialize_ctl_response),
            (Some("component"), Some("scale"), Some(_host_id), None) => Arc::clone(&self)
                .handle_scale_component(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("component"), Some("update"), Some(_host_id), None) => Arc::clone(&self)
                .handle_update_component(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Provider commands
            (Some("provider"), Some("auction"), None, None) => self
                .handle_auction_provider(message.payload)
                .await
                .map(serialize_ctl_response),
            (Some("provider"), Some("start"), Some(_host_id), None) => Arc::clone(&self)
                .handle_start_provider(message.payload)
                .await
                .map(serialize_ctl_response),
            (Some("provider"), Some("stop"), Some(_host_id), None) => self
                .handle_stop_provider(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Host commands
            (Some("host"), Some("get"), Some(_host_id), None) => self
                .handle_inventory()
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("host"), Some("ping"), None, None) => self
                .handle_ping_hosts()
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("host"), Some("stop"), Some(host_id), None) => self
                .handle_stop_host(message.payload, host_id)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Claims commands
            (Some("claims"), Some("get"), None, None) => self
                .handle_claims()
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Link commands
            (Some("link"), Some("del"), None, None) => self
                .handle_link_del(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("link"), Some("get"), None, None) => {
                // Explicitly returning a Vec<u8> for non-cloning efficiency within handle_links
                self.handle_links().await.map(|bytes| Some(Ok(bytes)))
            }
            (Some("link"), Some("put"), None, None) => self
                .handle_link_put(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Label commands
            (Some("label"), Some("del"), Some(host_id), None) => self
                .handle_label_del(host_id, message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("label"), Some("put"), Some(host_id), None) => self
                .handle_label_put(host_id, message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Registry commands
            (Some("registry"), Some("put"), None, None) => self
                .handle_registries_put(message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Config commands
            (Some("config"), Some("get"), Some(config_name), None) => self
                .handle_config_get(config_name)
                .await
                .map(|bytes| Some(Ok(bytes))),
            (Some("config"), Some("put"), Some(config_name), None) => self
                .handle_config_put(config_name, message.payload)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            (Some("config"), Some("del"), Some(config_name), None) => self
                .handle_config_delete(config_name)
                .await
                .map(Some)
                .map(serialize_ctl_response),
            // Topic fallback
            _ => {
                warn!(%subject, "received control interface request on unsupported subject");
                Ok(serialize_ctl_response(Some(CtlResponse::error(
                    "unsupported subject",
                ))))
            }
        };

        if let Err(err) = &ctl_response {
            error!(%subject, ?err, "failed to handle control interface request");
        } else {
            trace!(%subject, "handled control interface request");
        }

        match ctl_response {
            Ok(Some(Ok(payload))) => Some(payload.into()),
            // No response from the host (e.g. auctioning provider)
            Ok(None) => None,
            Err(e) => Some(
                serde_json::to_vec(&CtlResponse::error(&e.to_string()))
                    .context("failed to encode control interface response")
                    // This should never fail to serialize, but the fallback ensures that we send
                    // something back to the client even if we somehow fail.
                    .unwrap_or_else(|_| format!(r#"{{"success":false,"error":"{e}"}}"#).into())
                    .into(),
            ),
            // This would only occur if we failed to serialize a valid CtlResponse. This is
            // programmer error.
            Ok(Some(Err(e))) => Some(
                serde_json::to_vec(&CtlResponse::error(&e.to_string()))
                    .context("failed to encode control interface response")
                    .unwrap_or_else(|_| format!(r#"{{"success":false,"error":"{e}"}}"#).into())
                    .into(),
            ),
        }
    }
}

/// A control interface server that receives messages on the NATS message bus and
/// dispatches them to the host for processing.
pub struct NatsControlInterfaceServer {
    ctl_nats: Arc<async_nats::Client>,
    data_store: Store,
    ctl_topic_prefix: String,
    enable_component_auction: bool,
    enable_provider_auction: bool,
}

impl NatsControlInterfaceServer {
    /// Create a new NATS control interface server.
    ///
    /// # Arguments
    /// * `ctl_nats` - The NATS client to use for sending and receiving messages.
    /// * `data_store` - The JetStream KV bucket where ComponentSpecs are stored.
    /// * `ctl_topic_prefix` - The topic prefix to use for control interface messages.
    /// * `enable_component_auction` - Whether to enable component auctioning.
    /// * `enable_provider_auction` - Whether to enable provider auctioning.
    pub fn new(
        ctl_nats: async_nats::Client,
        data_store: Store,
        ctl_topic_prefix: String,
        enable_component_auction: bool,
        enable_provider_auction: bool,
    ) -> Self {
        Self {
            ctl_nats: Arc::new(ctl_nats),
            data_store,
            ctl_topic_prefix,
            enable_component_auction,
            enable_provider_auction,
        }
    }

    #[instrument(level = "trace", skip_all)]
    /// Start the control interface server, returning a JoinSet of tasks.
    /// This will start the NATS subscriber and the data watch tasks.
    pub async fn start(
        self,
        host: Arc<crate::wasmbus::Host>,
    ) -> anyhow::Result<JoinSet<anyhow::Result<()>>> {
        let queue = Queue::new(
            &self.ctl_nats,
            &self.ctl_topic_prefix,
            host.lattice(),
            &host.id(),
            self.enable_component_auction,
            self.enable_provider_auction,
        )
        .await
        .context("failed to initialize queue")?;

        let mut tasks = JoinSet::new();
        data_watch(&mut tasks, self.data_store, host.clone())
            .await
            .context("failed to start data watch")?;

        tasks.spawn({
            let ctl_nats = Arc::clone(&self.ctl_nats);
            let host = Arc::clone(&host);
            async move {
                queue
                    .for_each_concurrent(None, {
                        let host = Arc::clone(&host);
                        let ctl_nats = Arc::clone(&ctl_nats);
                        move |msg| {
                            let host = Arc::clone(&host);
                            let ctl_nats = Arc::clone(&ctl_nats);
                            async move {
                                let msg_subject = msg.subject.clone();
                                let msg_reply = msg.reply.clone();
                                let payload = host.handle_ctl_message(msg).await;
                                if let Some(reply) = msg_reply {
                                    // TODO(brooksmtownsend): parse subject here
                                    // TODO(brooksmtownsend): ensure this is instrumented properly
                                    let headers = injector_to_headers(&TraceContextInjector::default_with_span());
                                    if let Some(payload) = payload {
                                        let max_payload = ctl_nats.server_info().max_payload;
                                        if payload.len() > max_payload {
                                            warn!(
                                                size = payload.len(),
                                                max_size = max_payload,
                                                "ctl response payload is too large to publish and may fail",
                                            );
                                        }
                                        if let Err(err) =
                                            ctl_nats
                                            .publish_with_headers(reply.clone(), headers, payload)
                                            .err_into::<anyhow::Error>()
                                            .and_then(|()| ctl_nats.flush().err_into::<anyhow::Error>())
                                            .await
                                        {
                                            tracing::error!(%msg_subject, ?err, "failed to publish reply to control interface request");
                                        }
                                    }
                                }
                            }
                        }
                    })
                    .await;

                let deadline = { *host.stop_rx.borrow() };
                host.stop_tx.send_replace(deadline);
                Ok(())
            }
        });

        Ok(tasks)
    }
}

/// Helper function to serialize `CtlResponse`<T> into a Vec<u8> if the response is Some
fn serialize_ctl_response<T: Serialize>(
    ctl_response: Option<CtlResponse<T>>,
) -> Option<anyhow::Result<Vec<u8>>> {
    ctl_response.map(|resp| serde_json::to_vec(&resp).map_err(anyhow::Error::from))
}
