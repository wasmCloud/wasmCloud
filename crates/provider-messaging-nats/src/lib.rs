use core::time::Duration;

use std::collections::HashMap;
use std::sync::Arc;

use crate::bindings::ext::exports::wrpc::extension::{
    configurable::{self, InterfaceConfig},
    manageable,
};
use anyhow::{bail, ensure, Context as _};
use async_nats::subject::ToSubject;
use bytes::Bytes;
use futures::StreamExt as _;
use opentelemetry_nats::{attach_span_context, NatsHeaderInjector};
use tokio::fs;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, error, instrument};
use tracing_futures::Instrument;
use wascap::prelude::KeyPair;
use wasmcloud_core::messaging::ConnectionConfig;

use wasmcloud_provider_sdk::provider::WrpcClient;
use wasmcloud_provider_sdk::wasmcloud_tracing::context::TraceContextInjector;
use wasmcloud_provider_sdk::{
    get_connection, initialize_observability, propagate_trace_for_ctx, run_provider,
    serve_provider_exports,
    types::{BindRequest, BindResponse, HealthCheckResponse},
    Context,
};

mod connection;

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
#[derive(Clone)]
pub struct NatsMessagingProvider {
    handler_components: Arc<RwLock<HashMap<String, NatsClientBundle>>>,
    consumer_components: Arc<RwLock<HashMap<String, NatsClientBundle>>>,
    default_config: Arc<RwLock<ConnectionConfig>>,
    quit_tx: Arc<tokio::sync::broadcast::Sender<()>>,
}

impl NatsMessagingProvider {
    fn new(quit_tx: tokio::sync::broadcast::Sender<()>) -> Self {
        Self {
            handler_components: Arc::default(),
            consumer_components: Arc::default(),
            default_config: Arc::default(),
            quit_tx: Arc::new(quit_tx),
        }
    }
}

impl NatsMessagingProvider {
    pub fn name() -> &'static str {
        "messaging-nats-provider"
    }

    pub async fn run() -> anyhow::Result<()> {
        let (shutdown, quit_tx) = run_provider(Self::name(), None)
            .await
            .context("failed to run provider")?;
        let provider = NatsMessagingProvider::new(quit_tx);
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

impl manageable::Handler<Option<Context>> for NatsMessagingProvider {
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
        // clear the handler components
        let mut handlers = self.handler_components.write().await;
        handlers.clear();

        // clear the consumer components
        let mut consumers = self.consumer_components.write().await;
        consumers.clear();

        // dropping all connections should send unsubscribes and close the connections, so no need
        // to handle that here

        // Signal the provider to shut down
        let _ = self.quit_tx.send(());
        Ok(Ok(()))
    }
}

impl configurable::Handler<Option<Context>> for NatsMessagingProvider {
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
            .or_else(|| std::env::var("PROVIDER_NATS_MESSAGING_FLAMEGRAPH_PATH").ok());
        initialize_observability!(Self::name(), flamegraph_path, config.config);

        let config_map: HashMap<String, String> = config.config.into_iter().collect();
        let config = ConnectionConfig::from_map(&config_map);
        if let Result::Ok(config) = config {
            *self.default_config.write().await = config;
        }

        Ok(Ok(()))
    }

    #[instrument(level = "debug", skip_all, fields(source_id))]
    async fn update_interface_export_config(
        &self,
        _cx: Option<Context>,
        source_id: String,
        _link_name: String,
        link_config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config = if link_config.config.is_empty() {
            self.default_config.read().await.clone()
        } else {
            // create a config from the supplied values and merge that with the existing default
            match connection::from_link_config(link_config) {
                Result::Ok(cc) => self.default_config.read().await.merge(&ConnectionConfig {
                    subscriptions: Box::default(),
                    ..cc
                }),
                Result::Err(e) => {
                    error!("Failed to build connection configuration: {e:?}");
                    return Ok(Err(format!("failed to build connection config: {e}")));
                }
            }
        };

        let mut update_map = self.consumer_components.write().await;
        let bundle = match self.connect(config, &source_id).await {
            Result::Ok(b) => b,
            Result::Err(e) => {
                error!("Failed to connect to NATS: {e:?}");
                return Ok(Err(format!("failed to connect to NATS: {e}")));
            }
        };
        update_map.insert(source_id.into(), bundle);

        Ok(Ok(()))
    }

    async fn update_interface_import_config(
        &self,
        _cx: Option<Context>,
        target_id: String,
        _link_name: String,
        link_config: InterfaceConfig,
    ) -> anyhow::Result<Result<(), String>> {
        let config = if link_config.config.is_empty() {
            self.default_config.read().await.clone()
        } else {
            // create a config from the supplied values and merge that with the existing default
            match connection::from_link_config(link_config) {
                Result::Ok(cc) => self.default_config.read().await.merge(&cc),
                Result::Err(e) => {
                    error!("Failed to build connection configuration: {e:?}");
                    return Ok(Err(format!("failed to build connection config: {e}")));
                }
            }
        };

        let mut update_map = self.handler_components.write().await;
        let bundle = match self.connect(config, &target_id).await {
            Result::Ok(b) => b,
            Result::Err(e) => {
                error!("Failed to connect to NATS: {e:?}");
                return Ok(Err(format!("failed to connect to NATS: {e}")));
            }
        };
        update_map.insert(target_id.into(), bundle);
        Ok(Ok(()))
    }

    #[instrument(level = "info", skip_all, fields(target_id))]
    async fn delete_interface_import_config(
        &self,
        _cx: Option<Context>,
        target_id: String,
        _link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        let mut links = self.handler_components.write().await;
        if let Some(bundle) = links.remove(&target_id) {
            // Note: subscriptions will be closed via Drop on the NatsClientBundle
            let client = &bundle.client;
            debug!(
                target_id,
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
            target_id,
            "finished processing (handler) link deletion for component",
        );

        Ok(Ok(()))
    }

    #[instrument(level = "info", skip_all, fields(source_id))]
    async fn delete_interface_export_config(
        &self,
        _cx: Option<Context>,
        source_id: String,
        _link_name: String,
    ) -> anyhow::Result<Result<(), String>> {
        let mut links = self.consumer_components.write().await;
        if let Some(bundle) = links.remove(&source_id) {
            let client = &bundle.client;
            debug!(
                source_id,
                "dropping NATS client [{}] for (consumer) component",
                format!(
                    "{}:{}",
                    client.server_info().server_id,
                    client.server_info().client_id
                ),
            );
        }

        debug!(
            source_id,
            "finished processing (consumer) link deletion for component",
        );

        Ok(Ok(()))
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
