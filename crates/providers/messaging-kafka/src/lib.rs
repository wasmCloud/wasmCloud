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
use tracing::{debug, instrument, warn};

use wasmcloud_provider_wit_bindgen::deps::{
    async_trait::async_trait,
    wasmcloud_provider_sdk::core::LinkDefinition,
    wasmcloud_provider_sdk::error::{ProviderInvocationError, ProviderInvocationResult},
    wasmcloud_provider_sdk::Context,
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
    #[instrument(level = "info", skip(self))]
    async fn put_link(&self, ld: &LinkDefinition) -> bool {
        debug!("putting link for actor {ld:?}");
        // Collect comma separated hosts into a Vec<String>
        let hosts = ld
            .values
            .iter()
            .find_map(|(k, v)| {
                if k == KAFKA_HOSTS {
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
        let topic = ld
            .values
            .iter()
            .find_map(|(k, v)| {
                if k == KAFKA_TOPIC {
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
                "Could not create Kafka client for actor {}, messages won't be received",
                ld.actor_id
            );
            return true;
        };

        // Create a partition client
        let Ok(partition_client) = client
            .partition_client(&topic, 0, UnknownTopicHandling::Error)
            .await
        else {
            warn!(
                "Could not create partition client for actor {}, messages won't be received",
                ld.actor_id
            );
            return true;
        };

        // Clone for moving into thread
        let ld = ld.clone();
        let actor_id = ld.actor_id.clone();
        let join = tokio::task::spawn(async move {
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
                if let Err(e) = InvocationHandler::new(&ld)
                    .handle_message(Message {
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
            actor_id,
            KafkaConnection {
                consumer_handle: Arc::new(join),
                connection_hosts: hosts,
            },
        );

        true
    }

    /// Handle notification that a link is dropped: close the connection
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, actor_id: &str) {
        debug!("deleting link for actor {}", actor_id);

        let mut connections = self.connections.write().unwrap();
        if let Some(KafkaConnection {
            consumer_handle: handle,
            ..
        }) = connections.remove(actor_id)
        {
            handle.abort()
        } else {
            debug!("Linkdef deleted for non-existent consumer, ignoring")
        }
    }

    /// Handle shutdown request with any cleanup necessary
    async fn shutdown(&self) {
        self.connections
            .write()
            .expect("failed to write connections")
            .drain()
            .for_each(|(_actor_id, connection)| {
                connection.consumer_handle.abort();
            });
    }
}

/// Implement the 'wasmcloud:messaging' capability provider interface
#[async_trait]
impl WasmcloudMessagingMessaging for KafkaMessagingProvider {
    #[instrument(
        level = "debug", 
        skip_all,
        fields(subject = %msg.subject, reply_to = ?msg.reply_to, body_len = %msg.body.len())
    )]
    async fn publish(&self, ctx: Context, msg: Message) -> ProviderInvocationResult<()> {
        debug!("publishing message: {msg:?}");

        let hosts = {
            let connections = self.connections.read().map_err(|e| {
                ProviderInvocationError::Provider(format!("failed to read connections: {e}"))
            })?;

            let config = connections
                .get(&ctx.actor.clone().unwrap())
                .ok_or_else(|| {
                    ProviderInvocationError::Provider(
                        "failed to find actor for connection".to_string(),
                    )
                })?;

            config.connection_hosts.clone()
        };

        let client = ClientBuilder::new(hosts).build().await.map_err(|e| {
            ProviderInvocationError::Provider(format!("failed to build client: {e}"))
        })?;

        // Ensure topic exists
        let controller_client = client.controller_client().map_err(|e| {
            ProviderInvocationError::Provider(format!("failed to build controller client: {e}"))
        })?;

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
        let partition_client = client
            .partition_client(
                msg.subject.to_owned(),
                0, // partition
                UnknownTopicHandling::Error,
            )
            .await
            .map_err(|e| {
                ProviderInvocationError::Provider(format!("failed to create partition client: {e}"))
            })?;

        // produce some data
        let records = vec![Record {
            key: None,
            value: Some(msg.body.clone()),
            headers: BTreeMap::from([("source".to_owned(), b"wasm".to_vec())]),
            timestamp: chrono::offset::Utc::now(),
        }];

        partition_client
            .produce(records, Compression::default())
            .await
            .map_err(|e| {
                ProviderInvocationError::Provider(format!("failed to produce record: {e}"))
            })?;

        Ok(())
    }

    #[instrument(level = "debug", skip_all, fields(subject = %_msg.subject))]
    async fn request(
        &self,
        _ctx: Context,
        _msg: RequestMessage,
    ) -> ProviderInvocationResult<Message> {
        // Kafka does not support request-reply in the traditional sense. You can publish to a
        // topic, and get an acknowledgement that it was received, but you can't get a
        // reply from a consumer on the other side.

        Err(ProviderInvocationError::Provider(
            "not implemented (Kafka does not officially support the request-reply paradigm)".into(),
        ))
    }
}
