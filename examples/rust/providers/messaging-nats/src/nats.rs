use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use async_nats::subject::ToSubject;
use bytes::Bytes;
use futures::StreamExt;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, error};
use wasmcloud_provider_sdk::core::HostData;
use wasmcloud_provider_sdk::{
    get_connection, load_host_data, run_provider, serve_provider_exports, Context, LinkConfig,
    LinkDeleteInfo, Provider,
};

use crate::connection::ConnectionConfig;

mod bindings {
    wit_bindgen_wrpc::generate!({ generate_all });
}
use bindings::exports::wasmcloud::messaging::consumer::Handler;
use bindings::wasmcloud::messaging::types::BrokerMessage;

/// [`NatsClientBundle`]s hold a NATS client and information (subscriptions)
/// related to it.
///
/// This struct is necssary because subscriptions are *not* automatically removed on client drop,
/// meaning that we must keep track of all subscriptions to close once the client is done
#[derive(Debug)]
struct NatsClientBundle {
    #[allow(unused)]
    pub client: async_nats::Client,
    pub sub_handles: Vec<(String, JoinHandle<()>)>,
}

impl Drop for NatsClientBundle {
    fn drop(&mut self) {
        for handle in &self.sub_handles {
            handle.1.abort()
        }
    }
}

/// Nats implementation for the `wasmcloud:messaging` interface
#[derive(Default, Clone)]
pub struct NatsMessagingProvider {
    /// Map of NATS connection clients (including subscriptions) per component
    components: Arc<RwLock<HashMap<String, NatsClientBundle>>>,
    /// Default configuration to use when configuration is not provided on the link
    default_config: ConnectionConfig,
}

impl NatsMessagingProvider {
    /// Execute the provider, loading default configuration from the host and subscribing
    /// on the proper RPC topics via `wrpc::serve`
    pub async fn run() -> anyhow::Result<()> {
        let host_data = load_host_data().context("failed to load host data")?;
        let provider = Self::from_host_data(host_data);
        let shutdown = run_provider(provider.clone(), "nats-messaging-provider")
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        serve_provider_exports(
            &connection.get_wrpc_client(connection.provider_key()),
            provider,
            shutdown,
            bindings::serve,
        )
        .await
    }

    /// Build a [`NatsMessagingProvider`] from [`HostData`]
    pub fn from_host_data(host_data: &HostData) -> NatsMessagingProvider {
        let default_config = ConnectionConfig::from(&host_data.config);
        NatsMessagingProvider {
            default_config,
            ..Default::default()
        }
    }

    /// Attempt to connect to nats url for a component
    async fn connect(
        &self,
        cfg: ConnectionConfig,
        component_id: &str,
    ) -> anyhow::Result<NatsClientBundle> {
        let opts = async_nats::ConnectOptions::default();

        let client = opts
            .name("Example NATS Messaging Provider") // allow this to show up uniquely in a NATS connection list
            .connect(cfg.uri)
            .await?;

        // Subscribe to given subscription topics
        let mut sub_handles = Vec::new();
        for sub in cfg.subscriptions.iter().filter(|s| !s.is_empty()) {
            let (sub, queue) = match sub.split_once('|') {
                Some((sub, queue)) => (sub, Some(queue.to_string())),
                None => (sub.as_str(), None),
            };

            sub_handles.push((
                sub.to_string(),
                self.subscribe(&client, component_id, sub.to_string(), queue)
                    .await?,
            ));
        }

        Ok(NatsClientBundle {
            client,
            sub_handles,
        })
    }

    /// Add a subscription to the NATS client that listens for messages on the given subject
    /// and sends the message to the component to handle.
    async fn subscribe(
        &self,
        client: &async_nats::Client,
        component_id: &str,
        sub: impl ToSubject,
        queue: Option<String>,
    ) -> anyhow::Result<JoinHandle<()>> {
        let mut subscriber = match queue {
            Some(queue) => client.queue_subscribe(sub, queue).await,
            None => client.subscribe(sub).await,
        }?;

        debug!(?component_id, "spawning listener for component");
        let component_id = Arc::new(component_id.to_string());
        // Spawn a thread that listens for messages coming from NATS
        // this thread is expected to run the full duration that the provider is available
        let join_handle = tokio::spawn(async move {
            // Listen for NATS message(s)
            while let Some(msg) = subscriber.next().await {
                debug!(?msg, ?component_id, "received messsage");

                let component_id = Arc::clone(&component_id);
                // Dispatch message to component in a green thread / background task to avoid blocking
                tokio::spawn(async move { dispatch_msg(component_id.as_str(), msg).await });
            }
        });

        Ok(join_handle)
    }
}

async fn dispatch_msg(component_id: &str, nats_msg: async_nats::Message) {
    let msg = BrokerMessage {
        body: nats_msg.payload,
        reply_to: nats_msg.reply.map(|s| s.to_string()),
        subject: nats_msg.subject.to_string(),
    };
    debug!(
        subject = msg.subject,
        reply_to = ?msg.reply_to,
        component_id = component_id,
        "sending message to component",
    );

    // TODO: Send the message to the component's `wasmcloud:messaging/handler.handle-message` function
    todo!("Use wasmcloud:messaging/handler for NATS provider")
}

impl Provider for NatsMessagingProvider {
    /// This function is called when a new link is created between a component and this provider. For each
    /// linked component, the NATS provider should create a new connection to the NATS server and subscribe
    /// to the topics specified in the link configuration.
    ///
    /// When a new message is received on a subscribed topic, the provider should call the component's
    /// `wasmcloud:messaging/handler.handle-message` function to handle the message.
    async fn receive_link_config_as_target(
        &self,
        LinkConfig {
            source_id, config, ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let config = if config.is_empty() {
            self.default_config.clone()
        } else {
            self.default_config.merge(ConnectionConfig::from(config))
        };

        let mut update_map = self.components.write().await;
        let nats_bundle = match self.connect(config, source_id).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to connect to NATS: {e:?}");
                bail!(anyhow!(e).context("failed to connect to NATS"))
            }
        };
        update_map.insert(source_id.into(), nats_bundle);

        Ok(())
    }

    /// Handle notification that a link is dropped: close the connection which removes all subscriptions
    async fn delete_link_as_target(&self, link: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let source_id = link.get_source_id();
        let mut all_components = self.components.write().await;

        if all_components.remove(source_id).is_some() {
            // Note: subscriptions will be closed via Drop on the NatsClientBundle
            debug!(
                "closing NATS subscriptions for component [{}]...",
                source_id,
            );
        }

        debug!(
            "finished processing delete link for component [{}]",
            source_id
        );
        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> anyhow::Result<()> {
        let mut all_components = self.components.write().await;
        all_components.clear();
        Ok(())
    }
}

/// Implement the 'wasmcloud:messaging' capability provider interface
impl Handler<Option<Context>> for NatsMessagingProvider {
    // TODO: Implement `wasmcloud:messaging/consumer.publish` for the NATS provider
    /// Components will call this function to publish a message to a subject
    async fn publish(
        &self,
        _ctx: Option<Context>,
        _msg: BrokerMessage,
    ) -> anyhow::Result<Result<(), String>> {
        todo!("Implement wasmcloud:messaging/consumer.publish for NATS provider")
    }

    // TODO: Implement `wasmcloud:messaging/consumer.publish` for the NATS provider
    /// Components will call this function to publish a message to a subject and expect
    /// a response back
    async fn request(
        &self,
        _ctx: Option<Context>,
        _subject: String,
        _body: Bytes,
        _timeout_ms: u32,
    ) -> anyhow::Result<Result<BrokerMessage, String>> {
        todo!("Implement wasmcloud:messaging/consumer.request for NATS provider")
    }
}
