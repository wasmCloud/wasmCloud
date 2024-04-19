//! Implementation for wasmcloud:messaging
//!
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use anyhow::{bail, Context as _};
use futures::TryStreamExt as _;
use rskafka::client::consumer::{StartOffset, StreamConsumerBuilder};
use rskafka::client::partition::{Compression, UnknownTopicHandling};
use rskafka::client::ClientBuilder;
use rskafka::record::{Record, RecordAndOffset};
use tokio::spawn;
use tokio::task::JoinHandle;
use tracing::{debug, error, instrument, warn};
use wasmcloud_provider_sdk::{get_connection, run_provider, Context, LinkConfig, Provider};
use wasmcloud_tracing::context::TraceContextInjector;

use crate::wasmcloud::messaging::types::BrokerMessage;

wit_bindgen_wrpc::generate!();

/// Config value for hosts, accepted as a comma separated string
const KAFKA_HOSTS_CONFIG_KEY: &str = "hosts";
const DEFAULT_HOST: &str = "127.0.0.1:9092";

/// Config value for topic, accepted as a single string
const KAFKA_TOPIC_CONFIG_KEY: &str = "topic";
const DEFAULT_TOPIC: &str = "my-topic";

pub async fn run() -> anyhow::Result<()> {
    KafkaMessagingProvider::run().await
}

#[derive(Clone)]
/// A struct that contains a consumer task handler and the host connection strings
struct KafkaConnection {
    connection_hosts: Vec<String>,
    consumer_handle: Arc<JoinHandle<Result<(), rskafka::client::error::Error>>>,
}

#[derive(Clone, Default)]
pub struct KafkaMessagingProvider {
    // Map of component ID to the JoinHandle where messages are consumed. When a link is put
    // we spawn a tokio::task to handle messages, and on delete the task is closed
    connections: Arc<RwLock<HashMap<String, KafkaConnection>>>,
}

impl KafkaMessagingProvider {
    pub async fn run() -> anyhow::Result<()> {
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

impl Provider for KafkaMessagingProvider {
    #[instrument(skip_all, fields(source_id))]
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            source_id, config, ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        debug!("putting link for component [{source_id}]");

        // Collect comma separated hosts into a Vec<String>
        let hosts = config
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
            .collect::<Vec<String>>();

        // Retrieve or use default topic, trimming off extra whitespace
        let topic = config
            .iter()
            .find_map(|(k, v)| {
                if *k == KAFKA_TOPIC_CONFIG_KEY {
                    Some(v.as_str())
                } else {
                    None
                }
            })
            .unwrap_or(DEFAULT_TOPIC)
            .trim();

        // Do some basic validation before spawning off in a thread
        let Ok(client) = ClientBuilder::new(hosts.clone()).build().await else {
            warn!(
                source_id,
                "failed to create Kafka client for component, messages won't be received",
            );
            bail!("failed to create Kafka client for component, messages won't be received")
        };

        // Create a partition client
        let Ok(partition_client) = client
            .partition_client(topic, 0, UnknownTopicHandling::Error)
            .await
        else {
            warn!(
                source_id,
                "failed to create partition client for component, messages won't be received",
            );
            bail!("failed to create partition client for component, messages won't be received")
        };
        let partition_client = Arc::new(partition_client);

        let source_id: Arc<str> = source_id.into();
        let subject: Arc<str> = topic.into();
        // StartOffset::Latest only processes new messages, but Earliest will send every message.
        // This could be a linkdef tunable value in the future
        let join = spawn(
            StreamConsumerBuilder::new(partition_client, StartOffset::Latest)
                .with_max_wait_ms(100)
                .build()
                .try_filter_map(
                    |(
                        RecordAndOffset {
                            record: Record { value, .. },
                            ..
                        },
                        _water_mark,
                    )| async { Ok(value) },
                )
                .try_for_each({
                    let source_id = Arc::clone(&source_id);
                    move |message| {
                        let wrpc = get_connection().get_wrpc_client(&source_id);
                        let subject = Arc::clone(&subject);
                        async move {
                            if let Err(e) = wasmcloud::messaging::handler::handle_message(
                                &wrpc,
                                &BrokerMessage {
                                    body: message,
                                    // By default, we always append '.reply' for reply topics
                                    reply_to: Some(format!("{subject}.reply")),
                                    subject: subject.to_string(),
                                },
                            )
                            .await
                            {
                                eprintln!("Unable to send subscription: {e:?}");
                            }
                            Ok(())
                        }
                    }
                }),
        );

        let mut connections = self.connections.write().unwrap();
        connections.insert(
            source_id.to_string(),
            KafkaConnection {
                consumer_handle: Arc::new(join),
                connection_hosts: hosts,
            },
        );

        Ok(())
    }

    /// Handle notification that a link is dropped: close the connection
    #[instrument(skip(self))]
    async fn delete_link(&self, source_id: &str) -> anyhow::Result<()> {
        debug!("deleting link for component {}", source_id);

        let mut connections = self.connections.write().unwrap();
        if let Some(KafkaConnection {
            consumer_handle: handle,
            ..
        }) = connections.remove(source_id)
        {
            handle.abort();
        } else {
            debug!("Linkdef deleted for non-existent consumer, ignoring");
        }
        Ok(())
    }

    /// Handle shutdown request with any cleanup necessary
    async fn shutdown(&self) -> anyhow::Result<()> {
        self.connections
            .write()
            .expect("failed to write connections")
            .drain()
            .for_each(|(_source_id, connection)| {
                connection.consumer_handle.abort();
            });
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
    ) -> anyhow::Result<Result<(), String>> {
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

        let hosts = {
            let connections = match self.connections.read() {
                Ok(connections) => connections,
                Err(e) => {
                    error!(error = %e, "failed to read connections");
                    return Ok(Err(format!("failed to read connections: {e}")));
                }
            };

            let ctx = ctx.as_ref().context("context missing")?;
            let config = match connections.get(&ctx.component.clone().unwrap()) {
                Some(config) => config,
                None => {
                    error!("no component config for connection");
                    return Ok(Err("no component config for connection".to_string()));
                }
            };

            config.connection_hosts.clone()
        };

        // TODO: pool & reuse client(s)
        // Retrieve Kafka client
        let client = match ClientBuilder::new(hosts).build().await {
            Ok(client) => client,
            Err(e) => {
                error!(error = %e, "failed to build client");
                return Ok(Err(format!("failed to build client: {e}")));
            }
        };
        let controller_client = match client.controller_client() {
            Ok(controller_client) => controller_client,
            Err(e) => {
                error!(error = %e, "failed to build controller client");
                return Ok(Err(format!("failed to build controller client: {e}")));
            }
        };

        // Get the list of known topics
        let topics = match client.list_topics().await {
            Ok(topics) => topics,
            Err(e) => {
                error!(error = %e, "failed to list topics");
                return Ok(Err(format!("failed to list topics: {e}")));
            }
        };

        // Attempt to create the subject in question if not already present as a topic
        // TODO: accept linkdef tunable values for these
        if !topics.iter().any(|t| t.name == msg.subject) {
            if let Err(e) = controller_client
                .create_topic(
                    msg.subject.to_owned(),
                    1,     // partition
                    1,     // replication factor
                    1_000, // timeout (ms)
                )
                .await
            {
                warn!("could not create topic: {e:?}")
            }
        }

        // Get a partition-bound client
        let partition_client = match client
            .partition_client(
                msg.subject.clone(),
                0, // partition
                UnknownTopicHandling::Error,
            )
            .await
        {
            Ok(partition_client) => partition_client,
            Err(e) => {
                error!(error = %e, "failed to create partition client");
                return Ok(Err(format!("failed to create partition client: {e}")));
            }
        };

        // Produce some data
        let records = vec![Record {
            key: None,
            value: Some(msg.body),
            headers: BTreeMap::from([("source".to_owned(), b"wasm".to_vec())]),
            timestamp: chrono::offset::Utc::now(),
        }];

        if let Err(e) = partition_client
            .produce(records, Compression::default())
            .await
        {
            error!(error = %e, "failed to produce record");
            return Ok(Err(format!("failed to produce record: {e}")));
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
    ) -> anyhow::Result<Result<BrokerMessage, String>> {
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
