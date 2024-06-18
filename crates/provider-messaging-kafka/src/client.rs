use anyhow::{bail, Context as _, Result};
use futures::Stream;
use kafka::client::KafkaClient;
use kafka::consumer::{Builder as ConsumerBuilder, Consumer, Message};
use tokio::sync::oneshot::Sender;

/// An Async Kafka Client built on the [`tokio`] runtime
pub(crate) struct AsyncKafkaClient(pub(crate) KafkaClient);

impl AsyncKafkaClient {
    /// Build an [`AsyncKafkaClient`] for a list of hosts
    pub async fn from_hosts(hosts: Vec<String>) -> Result<Self> {
        Self::from_client(KafkaClient::new(hosts)).await
    }

    /// Build an [`AsyncKafkaClient`] from an existing [`KafkaClient`]
    pub async fn from_client(mut kc: KafkaClient) -> Result<Self> {
        let kc = tokio::task::spawn_blocking(move || {
            kc.load_metadata_all().context("failed to load metadata")?;
            Ok::<KafkaClient, anyhow::Error>(kc)
        })
        .await
        .context("failed to perform spawn blocking")?
        .context("failed to load metadata")?;
        Ok(Self(kc))
    }
}

/// An wrapper for easily using a [`kafka::consumer::Consumer`] asynchronously
pub(crate) struct AsyncKafkaConsumer(Consumer);

impl AsyncKafkaConsumer {
    /// Build from an [`AsyncKafkaClient`] which is guaranteed to have had metadata loaded at least once (during construction).
    pub async fn from_async_client(
        ac: AsyncKafkaClient,
        builder_fn: impl FnOnce(ConsumerBuilder) -> ConsumerBuilder,
    ) -> Result<Self> {
        let builder = builder_fn(Consumer::from_client(ac.0));
        let consumer = builder.create().context("failed to create consumer")?;
        Ok(Self(consumer))
    }

    /// Produce an unending stream of messages based on the inner consumer, with a mechanism for stopping
    pub async fn messages<'a>(&mut self) -> Result<(impl Stream<Item = Message<'a>>, Sender<()>)> {
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();

        // Listen forever for new messages with the consumer
        tokio::task::block_in_place(move || {
            let consumer = &mut self.0;
            loop {
                if let Ok(message_sets) = consumer.poll() {
                    for message_set in message_sets.iter() {
                        for _message in message_set.messages() {
                            // TODO: publish the message to the stream
                        }
                        if let Err(e) = consumer.consume_messageset(message_set) {
                            bail!("failed to consume message set: {e}");
                        }
                    }
                }

                // Commit all consumed stuff
                if let Err(e) = consumer.commit_consumed() {
                    bail!("failed to commit consumed info: {e}");
                }

                // If we've been told to stop, then we should stop
                if let Ok(()) = stop_rx.try_recv() {
                    return Ok(());
                }
            }
        })?;

        // TODO: actually listen forever, with some ability to stop
        Ok((futures::stream::empty(), stop_tx))
    }
}
