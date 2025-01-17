use core::time::Duration;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, ensure, Context as _};
use async_nats::subject::ToSubject;
use bytes::Bytes;
use futures::StreamExt as _;
use opentelemetry_nats::{attach_span_context, NatsHeaderInjector};
use tokio::fs;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, error, instrument, warn};
use tracing_futures::Instrument;
use wascap::prelude::KeyPair;
use wasmcloud_provider_sdk::core::HostData;
use wasmcloud_provider_sdk::provider::WrpcClient;
use wasmcloud_provider_sdk::wasmcloud_tracing::context::TraceContextInjector;
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, load_host_data, propagate_trace_for_ctx,
    run_provider, serve_provider_exports, Context, LinkConfig, LinkDeleteInfo, Provider,
};

mod connection;
pub use connection::{ConnectionConfig, ConsumerConfig};

mod bindings {
    wit_bindgen_wrpc::generate!({
        with: {
            "wasmcloud:messaging/consumer@0.2.0": generate,
            "wasmcloud:messaging/handler@0.2.0": generate,
            "wasmcloud:messaging/types@0.2.0": generate,
        },
    });
}
use bindings::wasmcloud::messaging::types::BrokerMessage;

pub async fn run() -> anyhow::Result<()> {
    NatsMessagingProvider::run().await
}

/// [`NatsClientBundle`]s hold a NATS client and information (subscriptions)
/// related to it.
///
/// This struct is necessary because subscriptions are *not* automatically removed on client drop,
/// meaning that we must keep track of all subscriptions to close once the client is done
#[derive(Debug)]
struct NatsClientBundle {
    pub client: async_nats::Client,
    pub sub_handles: Vec<(String, JoinHandle<()>)>,
}

impl Drop for NatsClientBundle {
    fn drop(&mut self) {
        for handle in &self.sub_handles {
            handle.1.abort();
        }
    }
}

/// Nats implementation for wasmcloud:messaging
#[derive(Default, Clone)]
pub struct NatsMessagingProvider {
    handler_components: Arc<RwLock<HashMap<String, NatsClientBundle>>>,
    consumer_components: Arc<RwLock<HashMap<String, NatsClientBundle>>>,
    default_config: ConnectionConfig,
}

impl NatsMessagingProvider {
    pub async fn run() -> anyhow::Result<()> {
        initialize_observability!(
            "nats-messaging-provider",
            std::env::var_os("PROVIDER_NATS_MESSAGING_FLAMEGRAPH_PATH")
        );

        let host_data = load_host_data().context("failed to load host data")?;
        let provider = Self::from_host_data(host_data);
        let shutdown = run_provider(provider.clone(), "messaging-nats-provider")
            .await
            .context("failed to run provider")?;
        let connection = get_connection();
        let wrpc = connection
            .get_wrpc_client(connection.provider_key())
            .await?;
        serve_provider_exports(&wrpc, provider, shutdown, bindings::serve)
            .await
            .context("failed to serve provider exports")
    }

    /// Build a [`NatsMessagingProvider`] from [`HostData`]
    pub fn from_host_data(host_data: &HostData) -> NatsMessagingProvider {
        let config = ConnectionConfig::from_map(&host_data.config);
        if let Ok(config) = config {
            NatsMessagingProvider {
                default_config: config,
                ..Default::default()
            }
        } else {
            warn!("Failed to build connection configuration, falling back to default");
            NatsMessagingProvider::default()
        }
    }

    /// Attempt to connect to nats url (with jwt credentials, if provided)
    async fn connect(
        &self,
        cfg: ConnectionConfig,
        component_id: &str,
    ) -> anyhow::Result<NatsClientBundle> {
        ensure!(
            cfg.consumers.is_empty(),
            "JetStream consumers not supported by this provider"
        );
        let mut opts = match (cfg.auth_jwt, cfg.auth_seed) {
            (Some(jwt), Some(seed)) => {
                let seed = KeyPair::from_seed(&seed).context("failed to parse seed key pair")?;
                let seed = Arc::new(seed);
                async_nats::ConnectOptions::with_jwt(jwt.into_string(), move |nonce| {
                    let seed = seed.clone();
                    async move { seed.sign(&nonce).map_err(async_nats::AuthError::new) }
                })
            }
            (None, None) => async_nats::ConnectOptions::default(),
            _ => bail!("must provide both jwt and seed for jwt authentication"),
        };
        if let Some(tls_ca) = cfg.tls_ca.as_deref() {
            opts = add_tls_ca(tls_ca, opts)?;
        } else if let Some(tls_ca_file) = cfg.tls_ca_file.as_deref() {
            let ca = fs::read_to_string(tls_ca_file)
                .await
                .context("failed to read TLS CA file")?;
            opts = add_tls_ca(&ca, opts)?;
        }

        // Use the first visible cluster_uri
        let url = cfg.cluster_uris.first().unwrap();

        // Override inbox prefix if specified
        if let Some(prefix) = cfg.custom_inbox_prefix {
            opts = opts.custom_inbox_prefix(prefix);
        }

        let client = opts
            .name("NATS Messaging Provider") // allow this to show up uniquely in a NATS connection list
            .connect(url.as_ref())
            .await?;

        // Connections
        let mut sub_handles = Vec::new();
        for sub in cfg.subscriptions.iter().filter(|s| !s.is_empty()) {
            let (sub, queue) = match sub.split_once('|') {
                Some((sub, queue)) => (sub, Some(queue.into())),
                None => (sub.as_str(), None),
            };

            sub_handles.push((
                sub.into(),
                self.subscribe(&client, component_id, sub.to_string(), queue)
                    .await?,
            ));
        }

        Ok(NatsClientBundle {
            client,
            sub_handles,
        })
    }

    /// Add a regular or queue subscription
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

        let component_id = Arc::from(component_id);
        // Spawn a thread that listens for messages coming from NATS
        // this thread is expected to run the full duration that the provider is available
        let join_handle = tokio::spawn(async move {
            let wrpc = match get_connection()
                .get_wrpc_client_custom(&component_id, None)
                .await
            {
                Ok(wrpc) => Arc::new(wrpc),
                Err(err) => {
                    error!(?err, "failed to construct wRPC client");
                    return;
                }
            };
            // Listen for NATS message(s)
            while let Some(msg) = subscriber.next().await {
                debug!(?msg, ?component_id, "received message");
                // Set up tracing context for the NATS message
                let span = tracing::debug_span!("handle_message", ?component_id);

                let component_id = Arc::clone(&component_id);
                let wrpc = Arc::clone(&wrpc);
                tokio::spawn(async move {
                    dispatch_msg(&wrpc, &component_id, msg)
                        .instrument(span)
                        .await;
                });
            }
        });

        Ok(join_handle)
    }
}

#[instrument(level = "debug", skip_all, fields(component_id = %component_id, subject = %nats_msg.subject, reply_to = ?nats_msg.reply))]
async fn dispatch_msg(wrpc: &WrpcClient, component_id: &str, nats_msg: async_nats::Message) {
    match nats_msg.headers {
        // If there are some headers on the message they might contain a span context
        // so attempt to attach them.
        Some(ref h) if !h.is_empty() => {
            attach_span_context(&nats_msg);
        }
        // Otherwise, we'll use the existing span context starting with this message
        _ => (),
    };

    let msg = BrokerMessage {
        body: nats_msg.payload,
        reply_to: nats_msg.reply.map(|s| s.into_string()),
        subject: nats_msg.subject.into_string(),
    };
    debug!(
        subject = msg.subject,
        reply_to = ?msg.reply_to,
        component_id = component_id,
        "sending message to component",
    );
    let mut cx = async_nats::HeaderMap::new();
    for (k, v) in TraceContextInjector::default_with_span().iter() {
        cx.insert(k.as_str(), v.as_str())
    }
    if let Err(e) =
        bindings::wasmcloud::messaging::handler::handle_message(wrpc, Some(cx), &msg).await
    {
        error!(
            error = %e,
            "Unable to send message"
        );
    }
}

/// Handle provider control commands
/// `put_link` (new component link command), `del_link` (remove link command), and shutdown
impl Provider for NatsMessagingProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-component resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn receive_link_config_as_target(
        &self,
        link_config: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let LinkConfig { source_id, .. } = link_config;
        let config = if link_config.config.is_empty() {
            self.default_config.clone()
        } else {
            // create a config from the supplied values and merge that with the existing default
            match ConnectionConfig::from_link_config(&link_config) {
                Ok(cc) => self.default_config.merge(&ConnectionConfig {
                    subscriptions: Box::default(),
                    ..cc
                }),
                Err(e) => {
                    error!("Failed to build connection configuration: {e:?}");
                    return Err(anyhow!(e).context("failed to build connection config"));
                }
            }
        };

        let mut update_map = self.consumer_components.write().await;
        let bundle = match self.connect(config, source_id).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to connect to NATS: {e:?}");
                bail!(anyhow!(e).context("failed to connect to NATS"))
            }
        };
        update_map.insert(source_id.into(), bundle);

        Ok(())
    }

    #[instrument(level = "debug", skip_all, fields(target_id))]
    async fn receive_link_config_as_source(
        &self,
        LinkConfig {
            target_id, config, ..
        }: LinkConfig<'_>,
    ) -> anyhow::Result<()> {
        let config = if config.is_empty() {
            self.default_config.clone()
        } else {
            // create a config from the supplied values and merge that with the existing default
            match ConnectionConfig::from_map(config) {
                Ok(cc) => self.default_config.merge(&cc),
                Err(e) => {
                    error!("Failed to build connection configuration: {e:?}");
                    return Err(anyhow!(e).context("failed to build connection config"));
                }
            }
        };

        let mut update_map = self.handler_components.write().await;
        let bundle = match self.connect(config, target_id).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to connect to NATS: {e:?}");
                bail!(anyhow!(e).context("failed to connect to NATS"))
            }
        };
        update_map.insert(target_id.into(), bundle);

        Ok(())
    }

    /// Handle notification that a link is dropped: close the connection
    #[instrument(level = "info", skip_all, fields(source_id = info.get_source_id()))]
    async fn delete_link_as_target(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        let component_id = info.get_source_id();
        let mut links = self.consumer_components.write().await;
        if let Some(bundle) = links.remove(component_id) {
            let client = &bundle.client;
            debug!(
                component_id,
                "dropping NATS client [{}] for (consumer) component",
                format!(
                    "{}:{}",
                    client.server_info().server_id,
                    client.server_info().client_id
                ),
            );
        }

        debug!(
            component_id,
            "finished processing (consumer) link deletion for component",
        );

        Ok(())
    }

    #[instrument(level = "info", skip_all, fields(target_id = info.get_source_id()))]
    async fn delete_link_as_source(&self, info: impl LinkDeleteInfo) -> anyhow::Result<()> {
        // If we were the source, then the component we're invoking is the target
        let component_id = info.get_target_id();
        let mut links = self.handler_components.write().await;
        if let Some(bundle) = links.remove(component_id) {
            // Note: subscriptions will be closed via Drop on the NatsClientBundle
            let client = &bundle.client;
            debug!(
                component_id,
                "dropping NATS client [{}] and associated subscriptions [{}] for (handler) component",
                format!(
                    "{}:{}",
                    client.server_info().server_id,
                    client.server_info().client_id
                ),
                &bundle.sub_handles.len(),
            );
        }

        debug!(
            component_id,
            "finished processing (handler) link deletion for component",
        );

        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> anyhow::Result<()> {
        // clear the handler components
        let mut handlers = self.handler_components.write().await;
        handlers.clear();

        // clear the consumer components
        let mut consumers = self.consumer_components.write().await;
        consumers.clear();

        // dropping all connections should send unsubscribes and close the connections, so no need
        // to handle that here
        Ok(())
    }
}

/// Implement the 'wasmcloud:messaging' capability provider interface
impl bindings::exports::wasmcloud::messaging::consumer::Handler<Option<Context>>
    for NatsMessagingProvider
{
    #[instrument(level = "debug", skip(self, ctx, msg), fields(subject = %msg.subject, reply_to = ?msg.reply_to, body_len = %msg.body.len()))]
    async fn publish(
        &self,
        ctx: Option<Context>,
        msg: BrokerMessage,
    ) -> anyhow::Result<Result<(), String>> {
        propagate_trace_for_ctx!(ctx);

        let nats_client =
            if let Some(ref source_id) = ctx.and_then(|Context { component, .. }| component) {
                let actors = self.consumer_components.read().await;
                let nats_bundle = match actors.get(source_id) {
                    Some(nats_bundle) => nats_bundle,
                    None => {
                        error!("component not linked: {source_id}");
                        bail!("component not linked: {source_id}")
                    }
                };
                nats_bundle.client.clone()
            } else {
                error!("no component in request");
                bail!("no component in request")
            };

        let headers = NatsHeaderInjector::default_with_span().into();

        let body = msg.body;
        let res = match msg.reply_to.clone() {
            Some(reply_to) => if should_strip_headers(&msg.subject) {
                nats_client
                    .publish_with_reply(msg.subject, reply_to, body)
                    .await
            } else {
                nats_client
                    .publish_with_reply_and_headers(msg.subject, reply_to, headers, body)
                    .await
            }
            .map_err(|e| e.to_string()),
            None => nats_client
                .publish_with_headers(msg.subject, headers, body)
                .await
                .map_err(|e| e.to_string()),
        };
        let _ = nats_client.flush().await;
        Ok(res)
    }

    #[instrument(level = "debug", skip(self, ctx), fields(subject = %subject))]
    async fn request(
        &self,
        ctx: Option<Context>,
        subject: String,
        body: Bytes,
        timeout_ms: u32,
    ) -> anyhow::Result<Result<BrokerMessage, String>> {
        let nats_client =
            if let Some(ref source_id) = ctx.and_then(|Context { component, .. }| component) {
                let actors = self.consumer_components.read().await;
                let nats_bundle = match actors.get(source_id) {
                    Some(nats_bundle) => nats_bundle,
                    None => {
                        error!("component not linked: {source_id}");
                        bail!("component not linked: {source_id}")
                    }
                };
                nats_bundle.client.clone()
            } else {
                error!("no component in request");
                bail!("no component in request")
            };

        // Inject OTEL headers
        let headers = NatsHeaderInjector::default_with_span().into();

        let timeout = Duration::from_millis(timeout_ms.into());
        // Perform the request with a timeout
        let request_with_timeout = if should_strip_headers(&subject) {
            tokio::time::timeout(timeout, nats_client.request(subject, body)).await
        } else {
            tokio::time::timeout(
                timeout,
                nats_client.request_with_headers(subject, headers, body),
            )
            .await
        };

        // Process results of request
        match request_with_timeout {
            Err(timeout_err) => {
                error!("nats request timed out: {timeout_err}");
                return Ok(Err(format!("nats request timed out: {timeout_err}")));
            }
            Ok(Err(send_err)) => {
                error!("nats send error: {send_err}");
                return Ok(Err(format!("nats send error: {send_err}")));
            }
            Ok(Ok(resp)) => Ok(Ok(BrokerMessage {
                body: resp.payload,
                reply_to: resp.reply.map(|s| s.into_string()),
                subject: resp.subject.into_string(),
            })),
        }
    }
}

// In the current version of the NATS server, using headers on certain $SYS.REQ topics will cause server-side
// parse failures
fn should_strip_headers(topic: &str) -> bool {
    topic.starts_with("$SYS")
}

pub fn add_tls_ca(
    tls_ca: &str,
    opts: async_nats::ConnectOptions,
) -> anyhow::Result<async_nats::ConnectOptions> {
    let ca = rustls_pemfile::read_one(&mut tls_ca.as_bytes()).context("failed to read CA")?;
    let mut roots = async_nats::rustls::RootCertStore::empty();
    if let Some(rustls_pemfile::Item::X509Certificate(ca)) = ca {
        roots.add_parsable_certificates([ca]);
    } else {
        bail!("tls ca: invalid certificate type, must be a DER encoded PEM file")
    };
    let tls_client = async_nats::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(opts.tls_client_config(tls_client).require_tls(true))
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_default_connection_serialize() {
        // test to verify that we can default a config with partial input
        let input = r#"
{
    "cluster_uris": ["nats://soyvuh"],
    "auth_jwt": "authy",
    "auth_seed": "seedy"
}
"#;

        let config: ConnectionConfig = serde_json::from_str(input).unwrap();
        assert_eq!(config.auth_jwt.unwrap().as_ref(), "authy");
        assert_eq!(config.auth_seed.unwrap().as_ref(), "seedy");
        assert_eq!(config.cluster_uris, [Box::from("nats://soyvuh")].into());
        assert_eq!(config.custom_inbox_prefix, None);
        assert!(config.subscriptions.is_empty());
        assert!(config.ping_interval_sec.is_none());
    }

    #[test]
    fn test_connectionconfig_merge() {
        // second > original, individual vec fields are replace not extend
        let cc1 = ConnectionConfig {
            cluster_uris: ["old_server".into()].into(),
            subscriptions: ["topic1".into()].into(),
            custom_inbox_prefix: Some("_NOPE.>".into()),
            ..Default::default()
        };
        let cc2 = ConnectionConfig {
            cluster_uris: ["server1".into(), "server2".into()].into(),
            auth_jwt: Some("jawty".into()),
            ..Default::default()
        };
        let cc3 = cc1.merge(&cc2);
        assert_eq!(cc3.cluster_uris, cc2.cluster_uris);
        assert_eq!(cc3.subscriptions, cc1.subscriptions);
        assert_eq!(cc3.auth_jwt, Some("jawty".into()));
        assert_eq!(cc3.custom_inbox_prefix, Some("_NOPE.>".into()));
    }

    #[test]
    fn test_from_map() -> anyhow::Result<()> {
        let cc = ConnectionConfig::from_map(&HashMap::from([(
            "custom_inbox_prefix".into(),
            "_TEST.>".into(),
        )]))?;
        assert_eq!(cc.custom_inbox_prefix, Some("_TEST.>".into()));
        Ok(())
    }
}
