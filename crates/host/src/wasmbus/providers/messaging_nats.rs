use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_nats::jetstream;
use futures::StreamExt;
use nkeys::{KeyPair, XKey};
use tokio::fs;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tracing::{debug, error, instrument, trace_span, warn, Instrument as _, Span};
use wasmcloud_core::HostData;
use wasmcloud_provider_messaging_nats::ConnectionConfig;
use wasmcloud_provider_messaging_nats::{add_tls_ca, ConsumerConfig};
use wasmcloud_provider_sdk::provider::{
    handle_provider_commands, receive_link_for_provider, ProviderCommandReceivers,
};
use wasmcloud_provider_sdk::{LinkConfig, LinkDeleteInfo, ProviderConnection};
use wasmcloud_runtime::capability::wrpc;
use wasmcloud_tracing::KeyValue;

use crate::wasmbus::{Component, InvocationContext};

struct Provider {
    config: ConnectionConfig,
    components: Arc<RwLock<HashMap<String, Arc<Component>>>>,
    messaging_links:
        Arc<RwLock<HashMap<Arc<str>, Arc<RwLock<HashMap<Box<str>, async_nats::Client>>>>>>,
    subscriptions: Mutex<HashMap<Arc<str>, HashMap<Box<str>, JoinSet<()>>>>,
    lattice_id: Arc<str>,
    host_id: Arc<str>,
}

impl Provider {
    async fn connect(
        &self,
        config: &HashMap<String, String>,
    ) -> anyhow::Result<(async_nats::Client, ConnectionConfig)> {
        // NOTE: Big part of this is copy-pasted from `provider-messaging-nats`
        let config = if config.is_empty() {
            self.config.clone()
        } else {
            match ConnectionConfig::from_map(config) {
                Ok(cc) => self.config.merge(&cc),
                Err(err) => {
                    error!(?err, "failed to build connection configuration");
                    return Err(anyhow!(err).context("failed to build connection config"));
                }
            }
        };
        let mut opts = match (&config.auth_jwt, &config.auth_seed) {
            (Some(jwt), Some(seed)) => {
                let seed = KeyPair::from_seed(seed).context("failed to parse seed key pair")?;
                let seed = Arc::new(seed);
                async_nats::ConnectOptions::with_jwt(jwt.to_string(), move |nonce| {
                    let seed = seed.clone();
                    async move { seed.sign(&nonce).map_err(async_nats::AuthError::new) }
                })
            }
            (None, None) => async_nats::ConnectOptions::default(),
            _ => bail!("must provide both jwt and seed for jwt authentication"),
        };
        if let Some(tls_ca) = config.tls_ca.as_deref() {
            opts = add_tls_ca(tls_ca, opts)?;
        } else if let Some(tls_ca_file) = config.tls_ca_file.as_deref() {
            let ca = fs::read_to_string(tls_ca_file)
                .await
                .context("failed to read TLS CA file")?;
            opts = add_tls_ca(&ca, opts)?;
        }

        // Use the first visible cluster_uri
        let url = config.cluster_uris.first().context("invalid address")?;

        // Override inbox prefix if specified
        if let Some(ref prefix) = config.custom_inbox_prefix {
            opts = opts.custom_inbox_prefix(prefix);
        }
        let nats = opts
            .name("builtin NATS Messaging Provider")
            .connect(url.as_ref())
            .await
            .context("failed to connect to NATS")?;
        Ok((nats, config))
    }
}

#[instrument(skip_all)]
async fn handle_message(
    components: Arc<RwLock<HashMap<String, Arc<Component>>>>,
    lattice_id: Arc<str>,
    host_id: Arc<str>,
    target_id: Arc<str>,
    msg: async_nats::Message,
) {
    use wrpc::exports::wasmcloud::messaging0_2_0::handler::Handler as _;

    opentelemetry_nats::attach_span_context(&msg);
    let component = {
        let components = components.read().await;
        let Some(component) = components.get(target_id.as_ref()) else {
            warn!(?target_id, "linked component not found");
            return;
        };
        Arc::clone(component)
    };
    let _permit = match component
        .permits
        .acquire()
        .instrument(trace_span!("acquire_message_permit"))
        .await
    {
        Ok(permit) => permit,
        Err(err) => {
            error!(?err, "failed to acquire execution permit");
            return;
        }
    };
    match component
        .instantiate(component.handler.copy_for_new(), component.events.clone())
        .handle_message(
            InvocationContext {
                span: Span::current(),
                start_at: Instant::now(),
                attributes: vec![
                    KeyValue::new("component.ref", Arc::clone(&component.image_reference)),
                    KeyValue::new("lattice", lattice_id),
                    KeyValue::new("host", host_id),
                ],
            },
            wrpc::wasmcloud::messaging0_2_0::types::BrokerMessage {
                subject: msg.subject.into_string(),
                body: msg.payload,
                reply_to: msg.reply.map(async_nats::Subject::into_string),
            },
        )
        .await
    {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            warn!(?err, "component failed to handle message")
        }
        Err(err) => {
            warn!(?err, "failed to call component")
        }
    }
}

impl wasmcloud_provider_sdk::Provider for Provider {
    #[instrument(level = "debug", skip_all)]
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            source_id,
            link_name,
            config,
            ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let (nats, _) = self.connect(config).await?;
        let mut links = self.messaging_links.write().await;
        let mut links = links.entry(source_id.into()).or_default().write().await;
        links.insert(link_name.into(), nats);
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn receive_link_config_as_source(
        &self,
        LinkConfig {
            target_id,
            config,
            link_name,
            ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let (nats, config) = self.connect(config).await?;
        let mut tasks = JoinSet::new();
        let target_id: Arc<str> = Arc::from(target_id);
        for ConsumerConfig {
            stream,
            consumer,
            max_messages,
            max_bytes,
        } in config.consumers
        {
            let js = jetstream::new(nats.clone());
            let stream = js
                .get_stream(stream)
                .await
                .context("failed to get stream")?;
            let consumer = stream
                .get_consumer(&consumer)
                .await
                .map_err(|err| anyhow!(err).context("failed to get consumer"))?;
            let sub = consumer.batch();
            let sub = if let Some(max_messages) = max_messages {
                sub.max_messages(max_messages)
            } else {
                sub
            };
            let sub = if let Some(max_bytes) = max_bytes {
                sub.max_bytes(max_bytes)
            } else {
                sub
            };
            let mut sub = sub.messages().await.context("failed to subscribe")?;

            let components = Arc::clone(&self.components);
            let lattice_id = Arc::clone(&self.lattice_id);
            let host_id = Arc::clone(&self.host_id);
            let target_id = Arc::clone(&target_id);
            tasks.spawn(async move {
                while let Some(msg) = sub.next().await {
                    let msg = match msg {
                        Ok(msg) => msg,
                        Err(err) => {
                            error!(?err, "failed to receive message");
                            continue;
                        }
                    };
                    let (msg, ack) = msg.split();
                    tokio::spawn(async move {
                        if let Err(err) = ack.ack().await {
                            error!(?err, "failed to ACK message");
                        } else {
                            debug!("successfully ACK'ed message")
                        }
                    });
                    tokio::spawn(handle_message(
                        Arc::clone(&components),
                        Arc::clone(&lattice_id),
                        Arc::clone(&host_id),
                        Arc::clone(&target_id),
                        msg,
                    ));
                }
            });
        }
        for sub in config.subscriptions {
            if sub.is_empty() {
                continue;
            }
            let mut sub = if let Some((subject, queue)) = sub.split_once('|') {
                nats.queue_subscribe(async_nats::Subject::from(subject), queue.into())
                    .await
            } else {
                nats.subscribe(sub).await
            }
            .context("failed to subscribe")?;
            let components = Arc::clone(&self.components);
            let lattice_id = Arc::clone(&self.lattice_id);
            let host_id = Arc::clone(&self.host_id);
            let target_id = Arc::clone(&target_id);
            tasks.spawn(async move {
                while let Some(msg) = sub.next().await {
                    tokio::spawn(handle_message(
                        Arc::clone(&components),
                        Arc::clone(&lattice_id),
                        Arc::clone(&host_id),
                        Arc::clone(&target_id),
                        msg,
                    ));
                }
            });
        }
        self.subscriptions
            .lock()
            .await
            .entry(target_id)
            .or_default()
            .insert(link_name.into(), tasks);
        Ok(())
    }

    #[instrument(level = "debug", skip_all)]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let target_id = info.get_target_id();
        let link_name = info.get_link_name();
        self.subscriptions
            .lock()
            .await
            .get_mut(target_id)
            .map(|links| links.remove(link_name));
        Ok(())
    }
}

impl crate::wasmbus::Host {
    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn start_messaging_nats_provider(
        &self,
        host_data: HostData,
        provider_xkey: XKey,
        provider_id: &str,
        host_id: &str,
    ) -> anyhow::Result<JoinSet<()>> {
        let mut tasks = JoinSet::new();
        let config =
            ConnectionConfig::from_map(&host_data.config).context("failed to parse config")?;

        let (quit_tx, quit_rx) = broadcast::channel(1);
        let commands = ProviderCommandReceivers::new(
            Arc::clone(&self.rpc_nats),
            &quit_tx,
            &self.host_config.lattice,
            provider_id,
            provider_id,
            host_id,
        )
        .await?;
        let conn = ProviderConnection::new(
            Arc::clone(&self.rpc_nats),
            Arc::from(provider_id),
            Arc::clone(&self.host_config.lattice),
            host_id.to_string(),
            host_data.config,
            provider_xkey,
            Arc::clone(&self.secrets_xkey),
        )
        .context("failed to establish provider connection")?;
        let provider = Provider {
            config,
            components: Arc::clone(&self.components),
            messaging_links: Arc::clone(&self.messaging_links),
            subscriptions: Mutex::default(),
            host_id: Arc::from(host_id),
            lattice_id: Arc::clone(&self.host_config.lattice),
        };
        for ld in host_data.link_definitions {
            if let Err(e) = receive_link_for_provider(&provider, &conn, ld).await {
                error!(
                    error = %e,
                    "failed to initialize link during provider startup",
                );
            }
        }
        tasks.spawn(async move {
            handle_provider_commands(provider, &conn, quit_rx, quit_tx, commands).await
        });

        Ok(tasks)
    }
}
