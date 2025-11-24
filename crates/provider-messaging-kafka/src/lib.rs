//! Implementation for wasmcloud:messaging

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::bindings::ext::exports::wrpc::extension::{
    configurable::{self, InterfaceConfig},
    manageable,
};
use anyhow::{bail, Context as _, Result};
use bytes::Bytes;
use kafka::producer::{Producer, Record};
use tokio::spawn;
use tokio::sync::oneshot::Sender;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tokio_stream::StreamExt;
use tracing::{debug, error, instrument, warn};
use wasmcloud_provider_sdk::core::secrets::SecretValue;
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, run_provider, serve_provider_exports,
    types::{BindRequest, BindResponse, HealthCheckResponse},
    Context,
};
use wasmcloud_tracing::context::TraceContextInjector;

mod client;
use client::{AsyncKafkaClient, AsyncKafkaConsumer};

mod bindings {
    wit_bindgen_wrpc::generate!({
        world: "interfaces",
        with: {
            "wasmcloud:messaging/consumer@0.2.0": generate,
            "wasmcloud:messaging/handler@0.2.0": generate,
            "wasmcloud:messaging/types@0.2.0": generate,
        },
    });

    pub mod ext {
        wit_bindgen_wrpc::generate!({
            world: "extension",
            with: {
                "wrpc:extension/types@0.0.1": wasmcloud_provider_sdk::types,
                "wrpc:extension/manageable@0.0.1": generate,
                "wrpc:extension/configurable@0.0.1": generate
            },
        });
    }
}
use bindings::wasmcloud::messaging::types::BrokerMessage;

/// Config value for hosts, accepted as a comma separated string
const KAFKA_HOSTS_CONFIG_KEY: &str = "hosts";
const DEFAULT_HOST: &str = "127.0.0.1:9092";

/// Config value for topic, accepted as a single string
const KAFKA_TOPIC_CONFIG_KEY: &str = "topic";
const DEFAULT_TOPIC: &str = "my-topic";

/// Config value for specifying a consumer group
const KAFKA_CONSUMER_GROUP_CONFIG_KEY: &str = "consumer_group";

/// Config value for specifying one or more comma delimited partition(s)
/// to use when consuming values
const KAFKA_CONSUMER_PARTITIONS_CONFIG_KEY: &str = "consumer_partitions";

/// Config value for specifying one or more comma delimited partition(s)
/// to use when producing values
const KAFKA_PRODUCER_PARTITIONS_CONFIG_KEY: &str = "producer_partitions";

/// Number of seconds to wait for a consumer to stop after triggering it
const CONSUMER_STOP_TIMEOUT_SECS: u64 = 5;

pub async fn run() -> Result<()> {
    KafkaMessagingProvider::run().await
}

/// A struct that contains a consumer task handler and the host connection strings
#[allow(dead_code)]
struct KafkaConnection {
    /// Hosts that the connection is using
    hosts: Vec<String>,
    /// Kafka client that can be used for one-off things
    client: AsyncKafkaClient,
    /// Handle to a tokio consumer task handle
    consumer: JoinHandle<anyhow::Result<()>>,
    /// Stop the consumer
    consumer_stop_tx: Sender<()>,
    /// Topic partition(s) on which the consumer is consuming messages
    consumer_partitions: Vec<i32>,
    /// Topic partition(s) on which the producer is sending messages
    producer_partitions: Vec<i32>,
    /// Consumer group
    consumer_group: Option<String>,
}

#[derive(Clone)]
pub struct KafkaMessagingProvider {
    // Map of Component ID to the JoinHandle where messages are consumed.
    //
    // When a link is put we spawn a tokio::task to handle messages, and on delete the task is closed
    connections: Arc<RwLock<HashMap<String, KafkaConnection>>>,
    quit_tx: Arc<tokio::sync::broadcast::Sender<()>>,
}

impl KafkaMessagingProvider {
    fn new(quit_tx: tokio::sync::broadcast::Sender<()>) -> Self {
        Self {
            connections: Arc::default(),
            quit_tx: Arc::new(quit_tx),
        }
    }
}

impl KafkaMessagingProvider {
    pub fn name() -> &'static str {
        "messaging-kafka-provider"
    }

    pub async fn run() -> anyhow::Result<()> {
        let (shutdown, quit_tx) = run_provider(KafkaMessagingProvider::name(), None)
            .await
            .context("failed to run provider")?;
        let provider = Self::new(quit_tx);
        let connection = get_connection();
        let (main_client, ext_client) = connection.get_wrpc_clients_for_serving().await?;
        serve_provider_exports(
            &main_client,
            &ext_client,
            provider,
            shutdown,
            bindings::serve,
            bindings::ext::serve,
        )
        .await
        .context("failed to serve provider exports")
    }
}

/// Extract hostnames (separated by commas, found under key [`KAFKA_HOSTS_CONFIG_KEY`]) from config hashmap
///
/// If no hostnames are found [`DEFAULT_HOST`] is split (by ',') and returned.
fn extract_hosts_from_link_config(link_config: &InterfaceConfig) -> Vec<String> {
    // Collect comma separated hosts into a Vec<String>
    //
    // This value could come from either secrets or regular config (for backwards compat)
    // but we want to make sure we warn if it is pulled from config.
    let maybe_hosts = link_config
        .secrets
        .as_ref()
        .and_then(|secrets| {
            secrets.iter().find_map(|(k, v)| {
                let secret: SecretValue = v.into();
                match (k.as_str(), secret.as_string()) {
                    (k, Some(v)) if k == KAFKA_HOSTS_CONFIG_KEY => Some(String::from(v)),
                    _ => None,
                }
            })
        })
        .or_else(|| {
            warn!("secret value [{KAFKA_HOSTS_CONFIG_KEY}] was not found in secrets. Prefer storing sensitive values in secrets");
            link_config
                .config
                .iter()
                .find_map(|(k, v)| {
                    if k == KAFKA_HOSTS_CONFIG_KEY {
                        Some(v.to_string())
                    } else {
                        None
                    }
                })
        });

    maybe_hosts
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

impl manageable::Handler<Option<Context>> for KafkaMessagingProvider {
    async fn bind(
        &self,
        _cx: Option<Context>,
        _req: BindRequest,
    ) -> anyhow::Result<Result<BindResponse, String>> {
        Ok(Ok(BindResponse {
            identity_token: None,
            provider_xkey: Some(get_connection().provider_xkey.public_key().into()),
        }))
    }

    async fn health_request(
        &self,
        _cx: Option<Context>,
    ) -> anyhow::Result<Result<HealthCheckResponse, String>> {
        Ok(Ok(HealthCheckResponse {
            healthy: true,
            message: Some("OK".to_string()),
        }))
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self, _cx: Option<Context>) -> anyhow::Result<Result<(), String>> {
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
        // Signal the provider to shut down
        let _ = self.quit_tx.send(());
        Ok(Ok(()))
    }
}

impl configurable::Handler<Option<Context>> for KafkaMessagingProvider {
    #[instrument(level = "debug", skip_all)]
    async fn update_base_config(
        &self,
        _cx: Option<Context>,
        config: wasmcloud_provider_sdk::types::BaseConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let flamegraph_path = config
            .config
            .iter()
            .find(|(k, _)| k == "FLAMEGRAPH_PATH")
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var("PROVIDER_MESSAGING_KAFKA_FLAMEGRAPH_PATH").ok());
        initialize_observability!(
            KafkaMessagingProvider::name(),
            flamegraph_path,
            config.config
        );

        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn update_interface_export_config(
        &self,
        _cx: Option<Context>,
        source_id: String,
        link_name: String,
        link_config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        debug!(link_name, source_id, "receiving link as target");
        // Convert config Vec to HashMap for easier access
        let config: HashMap<String, String> = link_config.config.iter().cloned().collect();
        // Collect various values from config (if present)
        let hosts = extract_hosts_from_link_config(&link_config);
        let topic = extract_topic_from_config(&config);
        let consumer_group = config
            .get(KAFKA_CONSUMER_GROUP_CONFIG_KEY)
            .map(String::to_string);
        let consumer_partitions = config
            .get(KAFKA_CONSUMER_PARTITIONS_CONFIG_KEY)
            .map(String::to_string)
            .unwrap_or_default()
            .split(',')
            .map(|s| s.into())
            .collect::<HashSet<String>>()
            .iter()
            .filter_map(|v| v.parse::<i32>().ok())
            .collect::<Vec<i32>>();
        let producer_partitions = config
            .get(KAFKA_PRODUCER_PARTITIONS_CONFIG_KEY)
            .map(String::to_string)
            .unwrap_or_default()
            .split(',')
            .map(|s| s.into())
            .collect::<HashSet<String>>()
            .iter()
            .filter_map(|v| v.parse::<i32>().ok())
            .collect::<Vec<i32>>();

        // Build client for use with the consumer
        let client = AsyncKafkaClient::from_hosts(hosts.clone()).await.with_context(|| {
            warn!(
                source_id,
                "failed to create Kafka client for component",
            );
            format!("failed to build async kafka client for component [{source_id}], messages won't be received")
        })?;

        // Build a consumer configured with our given client
        let _consumer_group = consumer_group.clone();
        let _consumer_partitions = consumer_partitions.clone();
        debug!(topic, ?consumer_partitions, "creating kafka async consumer");
        let consumer = AsyncKafkaConsumer::from_async_client(client, move |mut b| {
            b = b.with_topic(topic.into());
            b = b.with_topic_partitions(topic.into(), _consumer_partitions.as_slice());
            if let Some(g) = _consumer_group {
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
        let component_id: Arc<str> = source_id.clone().into();
        let subject: Arc<str> = topic.into();

        // Allow triggering listeners to stop
        let (stop_listener_tx, mut stop_listener_rx) = tokio::sync::oneshot::channel();

        // Start listening for incoming messages
        let (mut stream, inner_stop_tx) = match consumer
            .messages()
            .await
            .context("failed to start listening to consumer messages")
        {
            Ok(v) => v,
            Err(e) => {
                warn!("failed listening to consumer message stream: {e}");
                bail!(e);
            }
        };

        // StartOffset::Latest only processes new messages, but Earliest will send every message.
        // This could be a linkdef tunable value in the future
        let task = spawn(async move {
            let wrpc = get_connection().get_wrpc_client(&component_id).await?;

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
                        tokio::spawn(async move {
                            if let Err(e) = bindings::wasmcloud::messaging::handler::handle_message(
                                &wrpc,
                                None,
                                &BrokerMessage {
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
                consumer_partitions,
                producer_partitions,
                consumer_group,
            },
        );
        Ok(Ok(()))
    }

    async fn update_interface_import_config(
        &self,
        _cx: Option<Context>,
        _target_id: String,
        _link_name: String,
        _config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }

    async fn delete_interface_import_config(
        &self,
        _cx: Option<Context>,
        _target_id: String,
        _link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        Ok(Ok(()))
    }

    #[instrument(level = "info", skip_all, fields(source_id))]
    async fn delete_interface_export_config(
        &self,
        _cx: Option<Context>,
        source_id: String,
        _link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        debug!(source_id, "deleting link for component");

        // Find the connection and remove it from the HashMap
        let mut connections = self.connections.write().await;
        let Some(KafkaConnection {
            consumer,
            consumer_stop_tx,
            ..
        }) = connections.remove(&source_id)
        else {
            debug!("Linkdef deleted for non-existent consumer, ignoring");
            return Ok(Ok(()));
        };

        // Signal the consumer to stop, then wait for it to close out
        if let Err(()) = consumer_stop_tx.send(()) {
            bail!("failed to send stop consumer");
        }
        let _ = tokio::time::timeout(Duration::from_secs(CONSUMER_STOP_TIMEOUT_SECS), consumer)
            .await
            .context("consumer task did not exit cleanly")?;

        Ok(Ok(()))
    }
}

/// Implement the 'wasmcloud:messaging' capability provider interface
impl bindings::exports::wasmcloud::messaging::consumer::Handler<Option<Context>>
    for KafkaMessagingProvider
{
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
        let Some(KafkaConnection {
            hosts,
            producer_partitions,
            ..
        }) = connections.get(component_id)
        else {
            warn!(component_id, "failed to get connection for component");
            return Ok(Err(format!(
                "failed to get connection for component [{component_id}]"
            )));
        };

        // Create a producer we'll use to send
        let mut producer = Producer::from_hosts(hosts.clone())
            .create()
            .context("failed to build kafka producer")?;

        // For every partition we're listening on, send out a record
        // if we're listening on *no* partitions, then use the unspecified partition
        debug!(subject = msg.subject, "sending message");
        match producer_partitions[..] {
            // Send to the default ("unspecified") partition
            [] => {
                producer
                    .send(&Record::<(), Vec<u8>>::from_key_value(
                        &msg.subject,
                        (),
                        msg.body.to_vec(),
                    ))
                    .context("failed to send record")?;
            }
            // If there are multiple partitions to publish to, then publish to each of them
            _ => {
                for partition in producer_partitions {
                    producer
                        .send(
                            &Record::<(), Vec<u8>>::from_key_value(
                                &msg.subject,
                                (),
                                msg.body.to_vec(),
                            )
                            .with_partition(*partition),
                        )
                        .with_context(|| {
                            format!("failed to send record to partition [{partition}]")
                        })?;
                }
            }
        }

        Ok(Ok(()))
    }

    #[instrument(skip_all)]
    async fn request(
        &self,
        ctx: Option<Context>,
        _subject: String,
        _body: Bytes,
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
