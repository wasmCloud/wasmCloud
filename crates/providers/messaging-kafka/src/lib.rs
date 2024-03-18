//! Implementation for wasmcloud:messaging
//!
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use futures::StreamExt;
use rskafka::client::consumer::{StartOffset, StreamConsumerBuilder};
use rskafka::client::partition::{Compression, UnknownTopicHandling};
use rskafka::client::ClientBuilder;
use rskafka::record::{Record, RecordAndOffset};
use tokio::task::JoinHandle;
use tracing::{debug, error, instrument, warn};

use wasmcloud_provider_wit_bindgen::deps::{
    async_trait::async_trait,
    wasmcloud_provider_sdk::{Context, LinkConfig, ProviderOperationResult},
};

wasmcloud_provider_wit_bindgen::generate!({
    impl_struct: KafkaMessagingProvider,
    contract: "wasmcloud:messaging",
    wit_bindgen_cfg: "provider-messaging-kafka"
});

/// Linkdef value for hosts, accepted as a comma separated string
const KAFKA_HOSTS: &str = "HOSTS";
const DEFAULT_HOST: &str = "127.0.0.1:9092";

/// Linkdef value for topic, accepted as a single string
const KAFKA_TOPIC: &str = "TOPIC";
const DEFAULT_TOPIC: &str = "my-topic";

#[derive(Clone)]
/// A struct that contains a consumer task handler and the host connection strings
struct KafkaConnection {
    connection_hosts: Vec<String>,
    consumer_handle: Arc<JoinHandle<()>>,
}

#[derive(Clone, Default)]
pub struct KafkaMessagingProvider {
    // Map of actor ID to the JoinHandle where messages are consumed. When a link is put
    // we spawn a tokio::task to handle messages, and on delete the task is closed
    connections: Arc<RwLock<HashMap<String, KafkaConnection>>>,
}

#[async_trait]
impl WasmcloudCapabilityProvider for KafkaMessagingProvider {
    #[instrument(level = "info", skip_all, fields(source_id = %link_config.get_source_id()))]
    async fn receive_link_config_as_target(
        &self,
        link_config: impl LinkConfig,
    ) -> ProviderOperationResult<()> {
        let source_id = link_config.get_source_id();
        debug!("putting link for actor [{source_id}]");
        let config_values = link_config.get_config();

        // Collect comma separated hosts into a Vec<String>
        let hosts = config_values
            .iter()
            .find_map(|(k, v)| {
                if *k == KAFKA_HOSTS {
                    Some(v.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| DEFAULT_HOST.to_string())
            .trim()
            .split(',')
            .map(|s| s.to_string())
            .collect::<Vec<String>>();

        // Retrieve or use default topic, trimming off extra whitespace
        let topic = config_values
            .iter()
            .find_map(|(k, v)| {
                if *k == KAFKA_TOPIC {
                    Some(v.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| DEFAULT_TOPIC.to_string())
            .trim()
            .to_string();

        // Do some basic validation before spawning off in a thread
        let Ok(client) = ClientBuilder::new(hosts.clone()).build().await else {
            warn!(
                source_id,
                "failed to create Kafka client for actor, messages won't be received",
            );
            return Ok(());
        };

        // Create a partition client
        let Ok(partition_client) = client
            .partition_client(&topic, 0, UnknownTopicHandling::Error)
            .await
        else {
            warn!(
                source_id,
                "failed to create partition client for actor, messages won't be received",
            );
            return Ok(());
        };

        // Clone for moving into thread
        let component_id = source_id.clone();
        let join = tokio::task::spawn(async move {
            let component_id = component_id.clone();

            // construct stream consumer
            let mut stream =
            // StartOffset::Latest only processes new messages, but Earliest will send every message.
            // This could be a linkdef tunable value in the future
                StreamConsumerBuilder::new(Arc::new(partition_client), StartOffset::Latest)
                .with_max_wait_ms(100)
                .build();

            // Continue to pull records off the stream until it closes
            while let Some(Ok((
                RecordAndOffset {
                    record:
                        Record {
                            value: Some(message),
                            ..
                        },
                    ..
                },
                _water_mark,
            ))) = stream.next().await
            {
                let component_id = component_id.clone();
                if let Err(e) = InvocationHandler::new(component_id)
                    .handle_message(BrokerMessage {
                        body: message,
                        reply_to: None,
                        subject: topic.to_owned(),
                    })
                    .await
                {
                    eprintln!("Unable to send subscription: {:?}", e);
                }
            }
        });

        let mut connections = self.connections.write().unwrap();
        connections.insert(
            source_id.into(),
            KafkaConnection {
                consumer_handle: Arc::new(join),
                connection_hosts: hosts,
            },
        );

        Ok(())
    }

    /// Handle notification that a link is dropped: close the connection
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, source_id: &str) -> ProviderOperationResult<()> {
        debug!("deleting link for actor {}", source_id);

        let mut connections = self.connections.write().unwrap();
        if let Some(KafkaConnection {
            consumer_handle: handle,
            ..
        }) = connections.remove(source_id)
        {
            handle.abort()
        } else {
            debug!("Linkdef deleted for non-existent consumer, ignoring")
        }
        Ok(())
    }

    /// Handle shutdown request with any cleanup necessary
    async fn shutdown(&self) -> ProviderOperationResult<()> {
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
#[async_trait]
impl WasmcloudMessagingConsumer for KafkaMessagingProvider {
    #[instrument(
        level = "debug", 
        skip_all,
        fields(subject = %msg.subject, reply_to = ?msg.reply_to, body_len = %msg.body.len())
    )]
    async fn publish(&self, ctx: Context, msg: BrokerMessage) -> Result<(), String> {
        debug!("publishing message: {msg:?}");

        let hosts = {
            let connections = match self.connections.read() {
                Ok(connections) => connections,
                Err(e) => {
                    error!("failed to read connections: {e}");
                    return Err(format!("failed to read connections: {e}"));
                }
            };

            let config = match connections.get(&ctx.actor.clone().unwrap()) {
                Some(config) => config,
                None => {
                    error!("no actor config for connection");
                    return Err("no actor config for connection".to_string());
                }
            };

            config.connection_hosts.clone()
        };

        let client = match ClientBuilder::new(hosts).build().await {
            Ok(client) => client,
            Err(e) => {
                error!("failed to build client: {e}");
                return Err(format!("failed to build client: {e}"));
            }
        };

        // Ensure topic exists
        let controller_client = match client.controller_client() {
            Ok(controller_client) => controller_client,
            Err(e) => {
                error!("failed to build controller client: {e}");
                return Err(format!("failed to build controller client: {e}"));
            }
        };

        // TODO: accept linkdef tunable values for these
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

        // Get a partition-bound client
        let partition_client = match client
            .partition_client(
                msg.subject.to_owned(),
                0, // partition
                UnknownTopicHandling::Error,
            )
            .await
        {
            Ok(partition_client) => partition_client,
            Err(e) => {
                error!("failed to create partition client: {e}");
                return Err(format!("failed to create partition client: {e}"));
            }
        };

        // produce some data
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
            error!("failed to produce record: {e}");
            return Err(format!("failed to produce record: {e}"));
        }
        Ok(())
    }

    #[instrument(level = "debug", skip_all, fields(subject = _subject))]
    async fn request(
        &self,
        _ctx: Context,
        _subject: String,
        _body: Vec<u8>,
        _timeout_ms: u32,
    ) -> Result<BrokerMessage, String> {
        // Kafka does not support request-reply in the traditional sense. You can publish to a
        // topic, and get an acknowledgement that it was received, but you can't get a
        // reply from a consumer on the other side.
        error!("not implemented (Kafka does not officially support the request-reply paradigm)");
        Err(
            "not implemented (Kafka does not officially support the request-reply paradigm)"
                .to_string(),
        )
    }
}
