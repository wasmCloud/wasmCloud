//! Implementation for wasmcloud:messaging
//!
use std::{
    collections::{BTreeMap, HashMap},
    convert::Infallible,
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use futures::StreamExt;
use rskafka::{
    chrono::prelude::*,
    client::{
        consumer::{StartOffset, StreamConsumerBuilder},
        partition::{Compression, UnknownTopicHandling},
        ClientBuilder,
    },
    record::{Record, RecordAndOffset},
};
use tokio::task::JoinHandle;
use tracing::{debug, instrument, warn};
use wasmbus_rpc::{core::LinkDefinition, provider::prelude::*};
use wasmcloud_interface_messaging::{
    MessageSubscriber, MessageSubscriberSender, Messaging, MessagingReceiver, PubMessage,
    ReplyMessage, RequestMessage, SubMessage,
};

/// Linkdef value for hosts, accepted as a comma separated string
const KAFKA_HOSTS: &str = "HOSTS";
const DEFAULT_HOST: &str = "127.0.0.1:9092";
/// Linkdef value for topic, accepted as a single string
const KAFKA_TOPIC: &str = "TOPIC";
const DEFAULT_TOPIC: &str = "my-topic";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    provider_main(
        KafkaProvider::default(),
        Some("wasmCloud Kafka API Client Provider".to_string()),
    )?;

    eprintln!("Kafka provider exiting");
    Ok(())
}

#[derive(Clone)]
/// A struct that contains a consumer task handler and the host connection strings
struct KafkaConnection {
    connection_hosts: Vec<String>,
    consumer_handle: Arc<JoinHandle<()>>,
}

#[derive(Clone, Provider)]
#[services(Messaging)]
struct KafkaProvider {
    // Map of actor ID to the JoinHandle where messages are consumed. When a link is put
    // we spawn a tokio::task to handle messages, and on delete the task is closed
    connections: Arc<RwLock<HashMap<String, KafkaConnection>>>,
}

impl Default for KafkaProvider {
    fn default() -> Self {
        KafkaProvider {
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}
// use default implementations of provider message handlers
impl ProviderDispatch for KafkaProvider {}

#[async_trait]
impl ProviderHandler for KafkaProvider {
    #[instrument(level = "info", skip(self))]
    async fn put_link(&self, ld: &LinkDefinition) -> RpcResult<bool> {
        debug!("putting link for actor {ld:?}");
        // Collect comma separated hosts into a Vec<String>
        let hosts = ld
            .values
            .get(KAFKA_HOSTS)
            .cloned()
            .unwrap_or_else(|| DEFAULT_HOST.to_string())
            .trim()
            .split(',')
            .map(|s| s.to_string())
            .collect::<Vec<String>>();

        // Retrieve or use default topic, trimming off extra whitespace
        let topic = ld
            .values
            .get(KAFKA_TOPIC)
            .cloned()
            .unwrap_or_else(|| DEFAULT_TOPIC.to_string())
            .trim()
            .to_string();

        // Do some basic validation before spawning off in a thread
        if let Ok(client) = ClientBuilder::new(hosts.clone()).build().await {
            if let Ok(partition_client) = client
                .partition_client(&topic, 0, UnknownTopicHandling::Error)
                .await
            {
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
                        let actor = MessageSubscriberSender::for_actor(&ld);
                        if let Err(e) = actor
                            .handle_message(
                                &Context::default(),
                                &SubMessage {
                                    body: message,
                                    reply_to: None,
                                    subject: topic.to_owned(),
                                },
                            )
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
            } else {
                warn!(
                    "Could not create partition client for actor {}, messages won't be received",
                    ld.actor_id
                );
            }
        } else {
            warn!(
                "Could not create Kafka client for actor {}, messages won't be received",
                ld.actor_id
            );
        }

        Ok(true)
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
    async fn shutdown(&self) -> std::result::Result<(), Infallible> {
        let mut connections = self.connections.write().unwrap();

        connections.drain().for_each(|(_actor_id, connection)| {
            connection.consumer_handle.abort();
        });
        Ok(())
    }
}

/// Handle Messaging methods
#[async_trait]
impl Messaging for KafkaProvider {
    #[instrument(level = "debug", skip(self, msg), fields(subject = %msg.subject, reply_to = ?msg.reply_to, body_len = %msg.body.len()))]
    async fn publish(&self, ctx: &Context, msg: &PubMessage) -> RpcResult<()> {
        debug!("Publishing message: {:?}", msg);
        //TODO: kill these unwraps
        let config = self
            .connections
            .read()
            .unwrap()
            .get(&ctx.actor.clone().unwrap())
            .cloned()
            .unwrap();

        let client = ClientBuilder::new(config.connection_hosts)
            .build()
            .await
            .unwrap();

        // Ensure topic exists
        let controller_client = client.controller_client().unwrap();
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
            warn!("could not create topic: {:?}", e)
        }

        // get a partition-bound client
        let partition_client = client
            .partition_client(
                msg.subject.to_owned(),
                0, // partition
                UnknownTopicHandling::Error,
            )
            .await
            .unwrap();

        // produce some data
        let record = Record {
            key: None,
            value: Some(msg.body.clone()),
            headers: BTreeMap::from([("source".to_owned(), b"wasm".to_vec())]),
            timestamp: utc_now(),
        };
        partition_client
            .produce(vec![record], Compression::default())
            .await
            .unwrap();
        Ok(())
    }

    #[instrument(level = "debug", skip(self, msg), fields(subject = %msg.subject))]
    async fn request(&self, _ctx: &Context, msg: &RequestMessage) -> RpcResult<ReplyMessage> {
        // Kafka does not support request-reply in the traditional sense. You can publish to a
        // topic, and get an acknowledgement that it was received, but you can't get a
        // reply from a consumer on the other side.

        Err(RpcError::NotImplemented)
    }
}

// I have no idea why Utc::now() isn't in scope here, but this works for now
fn utc_now() -> DateTime<Utc> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before Unix epoch");
    let naive =
        NaiveDateTime::from_timestamp_opt(now.as_secs() as i64, now.subsec_nanos()).unwrap();
    DateTime::from_utc(naive, Utc)
}
