//! Implementation for wasmcloud:messaging
//!
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Context as _, Result};
use kafka::producer::{Producer, Record};
use tokio::spawn;
use tokio::sync::oneshot::Sender;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tokio_stream::StreamExt;
use tracing::{debug, error, instrument, warn};
use wasmcloud_provider_sdk::{get_connection, run_provider, Context, LinkConfig, Provider};
use wasmcloud_tracing::context::TraceContextInjector;

mod client;
use client::{AsyncKafkaClient, AsyncKafkaConsumer};

use crate::wasmcloud::messaging::types::BrokerMessage;

wit_bindgen_wrpc::generate!();

/// Config value for hosts, accepted as a comma separated string
const KAFKA_HOSTS_CONFIG_KEY: &str = "hosts";
const DEFAULT_HOST: &str = "127.0.0.1:9092";

/// Config value for topic, accepted as a single string
const KAFKA_TOPIC_CONFIG_KEY: &str = "topic";
const DEFAULT_TOPIC: &str = "my-topic";

/// Config value for specifying a consumer group
const KAFKA_CONSUMER_GROUP_CONFIG_KEY: &str = "consumer-group";

/// Number of seconds to wait for a consumer to stop after triggering it
const CONSUMER_STOP_TIMEOUT_SECS: u64 = 5;

pub async fn run() -> Result<()> {
    KafkaMessagingProvider::run().await
}

/// A struct that contains a consumer task handler and the host connection strings
struct KafkaConnection {
    /// Hosts that the connection is using
    hosts: Vec<String>,
    /// Kafka client that can be used for one-off things
    client: AsyncKafkaClient,
    /// Handle to a tokio consumer task handle
    consumer: JoinHandle<anyhow::Result<()>>,
    /// Stop the consumer
    consumer_stop_tx: Sender<()>,
}

#[derive(Clone, Default)]
pub struct KafkaMessagingProvider {
    // Map of Component ID to the JoinHandle where messages are consumed.
    //
    // When a link is put we spawn a tokio::task to handle messages, and on delete the task is closed
    connections: Arc<RwLock<HashMap<String, KafkaConnection>>>,
}

impl KafkaMessagingProvider {
    pub async fn run() -> Result<()> {
        let provider = Self::default();
        let shutdown = run_provider(provider.clone(), "messaging-kafka-provider")
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        serve(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
            shutdown,
        )
        .await
    }
}

/// Extract hostnames (separated by commas, found under key [`KAFKA_HOSTS_CONFIG_KEY`]) from config hashmap
///
/// If no hostnames are found [`DEFAULT_HOST`] is split (by ',') and returned.
fn extract_hosts_from_config(config: &HashMap<String, String>) -> Vec<String> {
    // Collect comma separated hosts into a Vec<String>
    config
        .iter()
        .find_map(|(k, v)| {
            if *k == KAFKA_HOSTS_CONFIG_KEY {
                Some(v.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| DEFAULT_HOST.to_string())
        .trim()
        .split(',')
        .map(std::string::ToString::to_string)
        .collect::<Vec<String>>()
}

/// Extract a topic (found under key [`KAFKA_TOPIC_CONFIG_KEY`]) from config hashmap
///
/// If no topic is found, [`DEFAULT_TOPIC`] is returned.
fn extract_topic_from_config(config: &HashMap<String, String>) -> &str {
    config
        .iter()
        .find_map(|(k, v)| {
            if *k == KAFKA_TOPIC_CONFIG_KEY {
                Some(v.as_str())
            } else {
                None
            }
        })
        .unwrap_or(DEFAULT_TOPIC)
        .trim()
}

impl Provider for KafkaMessagingProvider {
    /// Called when this provider is linked to, when the provider is the *target* of the link.
    #[instrument(skip_all, fields(source_id))]
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            link_name,
            source_id,
            config,
            ..
        }: LinkConfig<'_>,
    ) -> Result<()> {
        debug!(link_name, source_id, "receiving link as target");

        // Collect various values from config (if present)
        let hosts = extract_hosts_from_config(config);
        let topic = extract_topic_from_config(config);
        let consumer_group = config
            .get(KAFKA_CONSUMER_GROUP_CONFIG_KEY)
            .map(String::to_string);

        // Build client for use with the consumer
        let client = AsyncKafkaClient::from_hosts(hosts.clone()).await.with_context(|| {
            warn!(
                source_id,
                "failed to create Kafka client for component",
            );
            format!("failed to build async kafka client for component [{source_id}], messages won't be received")
        })?;

        // Build a consumer configured with our given client
        let mut consumer = AsyncKafkaConsumer::from_async_client(client, move |mut b| {
            b = b.with_topic(topic.into());
            if let Some(g) = consumer_group {
                b = b.with_group(g);
            }
            b
        }).await.with_context(|| {
            warn!(
                source_id,
                "failed to build consumer from Kafka client for component",
            );
            format!("failed to build consumer from kafka client for component [{source_id}], messages won't be received")
        })?;

        // Build a second client to store in the connection
        let client = AsyncKafkaClient::from_hosts(hosts.clone()).await.with_context(|| {
            warn!(
                source_id,
                "failed to create Kafka client for component",
            );
            format!("failed to build async kafka client for component [{source_id}], messages won't be received")
        })?;

        // Store reusable information for use when processing new messages
        let component_id: Arc<str> = source_id.into();
        let subject: Arc<str> = topic.into();

        // Allow triggering listeners to stop
        let (stop_listener_tx, mut stop_listener_rx) = tokio::sync::oneshot::channel();

        // StartOffset::Latest only processes new messages, but Earliest will send every message.
        // This could be a linkdef tunable value in the future
        let task = spawn(async move {
            let (mut stream, inner_stop_tx) = consumer
                .messages()
                .await
                .context("failed to start listening to consumer messages")?;

            let wrpc = get_connection().get_wrpc_client(&component_id);

            // Listen to messages forever until we're instructed to stop
            loop {
                tokio::select! {
                    // Handle listening to calls to stop
                    _ = &mut stop_listener_rx => {
                        if let Err(()) = inner_stop_tx.send(()) {
                            bail!("failed to send stop consumer");
                        }
                        return Ok(());
                    },

                    // Listen to the next messages in the stream
                    //
                    // This stream will essentially never stop producing values.
                    Some(msg) = stream.next() => {
                        let component_id = Arc::clone(&component_id);
                        let wrpc = wrpc.clone();
                        let subject = Arc::clone(&subject);

                        // Spawn off a
                        tokio::spawn(async move {
                            if let Err(e) = wasmcloud::messaging::handler::handle_message(
                                &wrpc,
                                &BrokerMessage {
                                    //body: message,
                                    body: msg.value.into(),
                                    // By default, we always append '.reply' for reply topics
                                    reply_to: Some(format!("{subject}.reply")),
                                    subject: subject.to_string(),
                                },
                            )
                                .await
                            {
                                warn!(
                                    subject = subject.to_string(),
                                    component_id = component_id.to_string(),
                                    "unable to send subscription: {e:?}",
                                );
                            }
                        });
                    }
                }
            }
        });

        // Save the newly task that constantly listens for messages to the provider
        let mut connections = self.connections.write().await;
        connections.insert(
            source_id.to_string(),
            KafkaConnection {
                client,
                consumer: task,
                consumer_stop_tx: stop_listener_tx,
                hosts,
            },
        );

        Ok(())
    }

    /// Handle notification that a link is dropped: close the connection
    #[instrument(skip(self))]
    async fn delete_link(&self, source_id: &str) -> Result<()> {
        debug!("deleting link for component {}", source_id);

        // Find the connection and remove it from the HashMap
        let mut connections = self.connections.write().await;
        let Some(KafkaConnection {
            consumer,
            consumer_stop_tx,
            ..
        }) = connections.remove(source_id)
        else {
            debug!("Linkdef deleted for non-existent consumer, ignoring");
            return Ok(());
        };

        // Signal the consumer to stop, then wait for it to close out
        if let Err(()) = consumer_stop_tx.send(()) {
            bail!("failed to send stop consumer");
        }
        let _ = tokio::time::timeout(Duration::from_secs(CONSUMER_STOP_TIMEOUT_SECS), consumer)
            .await
            .context("consumer task did not exit cleanly")?;

        Ok(())
    }

    /// Handle shutdown request with any cleanup necessary
    async fn shutdown(&self) -> Result<()> {
        let mut connections = self.connections.write().await;
        for (
            _source_id,
            KafkaConnection {
                consumer,
                consumer_stop_tx,
                ..
            },
        ) in connections.drain()
        {
            consumer_stop_tx
                .send(())
                .map_err(|_| anyhow::anyhow!("failed to send consumer stop"))?;
            if let Err(err) =
                tokio::try_join!(consumer).context("consumer task did not exit cleanly")
            {
                error!(?err, "failed to stop consumer task cleanly");
            };
        }
        Ok(())
    }
}

/// Implement the 'wasmcloud:messaging' capability provider interface
impl exports::wasmcloud::messaging::consumer::Handler<Option<Context>> for KafkaMessagingProvider {
    #[instrument(
        skip_all,
        fields(subject = %msg.subject, reply_to = ?msg.reply_to, body_len = %msg.body.len())
    )]
    async fn publish(
        &self,
        ctx: Option<Context>,
        msg: BrokerMessage,
    ) -> Result<std::result::Result<(), String>> {
        // Extract tracing information from invocation context, if present
        let trace_ctx = match ctx {
            Some(Context { ref tracing, .. }) if !tracing.is_empty() => tracing
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<Vec<(String, String)>>(),

            _ => TraceContextInjector::default_with_span()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        wasmcloud_tracing::context::attach_span_context(&trace_ctx);
        debug!(?msg, "publishing message");

        let ctx = ctx.as_ref().context("unexpectedly missing context")?;
        let Some(component_id) = ctx.component.as_ref() else {
            bail!("context unexpectedly missing component ID");
        };

        // Retrieve a usable Kafka client from the kafka connection for our component
        let connections = self.connections.read().await;
        let Some(KafkaConnection { client, hosts, .. }) = connections.get(component_id) else {
            warn!(component_id, "failed to get connection for component");
            return Ok(Err(format!(
                "failed to get connection for component [{component_id}]"
            )));
        };

        // Get the list of known topics
        let topics = client
            .0
            .topics()
            .iter()
            .map(|t| t.name().to_string())
            .collect::<Vec<String>>();

        // Create a producer
        let mut producer = Producer::from_hosts(hosts.clone())
            .create()
            .context("failed to build kafka producer")?;

        for topic in topics {
            producer
                .send(
                    &Record::<(), Vec<u8>>::from_key_value(&topic, (), msg.body.clone())
                        .with_partition(0),
                )
                .context("failed to send record")?;
        }

        Ok(Ok(()))
    }

    #[instrument(skip_all)]
    async fn request(
        &self,
        ctx: Option<Context>,
        _subject: String,
        _body: Vec<u8>,
        _timeout_ms: u32,
    ) -> Result<std::result::Result<BrokerMessage, String>> {
        // Extract tracing information from invocation context, if present
        let trace_ctx = match ctx {
            Some(Context { ref tracing, .. }) if !tracing.is_empty() => tracing
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<Vec<(String, String)>>(),

            _ => TraceContextInjector::default_with_span()
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        };
        wasmcloud_tracing::context::attach_span_context(&trace_ctx);

        // Kafka does not support request-reply in the traditional sense. You can publish to a
        // topic, and get an acknowledgement that it was received, but you can't get a
        // reply from a consumer on the other side.
        error!("not implemented (Kafka does not officially support the request-reply paradigm)");
        Ok(Err(
            "not implemented (Kafka does not officially support the request-reply paradigm)"
                .to_string(),
        ))
    }
}
