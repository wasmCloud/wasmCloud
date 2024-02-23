//! Nats implementation for wasmcloud:messaging.
//!
use std::{collections::HashMap, convert::Infallible, sync::Arc, time::Duration};

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, error, instrument, warn};
use tracing_futures::Instrument;
use wascap::prelude::KeyPair;
use wasmbus_rpc::{
    core::{HostData, LinkDefinition},
    otel::OtelHeaderInjector,
    provider::prelude::*,
};
use wasmcloud_interface_messaging::{
    MessageSubscriber, MessageSubscriberSender, Messaging, MessagingReceiver, PubMessage,
    ReplyMessage, RequestMessage, SubMessage,
};

const DEFAULT_NATS_URI: &str = "0.0.0.0:4222";
const ENV_NATS_SUBSCRIPTION: &str = "SUBSCRIPTION";
const ENV_NATS_URI: &str = "URI";
const ENV_NATS_CLIENT_JWT: &str = "CLIENT_JWT";
const ENV_NATS_CLIENT_SEED: &str = "CLIENT_SEED";
const ENV_NATS_TLS_CA: &str = "TLS_CA";
const ENV_NATS_TLS_CA_FILE: &str = "TLS_CA_FILE";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // handle lattice control messages and forward rpc to the provider dispatch
    // returns when provider receives a shutdown control message
    let host_data = load_host_data()?;
    let provider = generate_provider(host_data);
    provider_main(provider, Some("NATS Messaging Provider".to_string()))?;

    eprintln!("NATS messaging provider exiting");
    Ok(())
}

fn generate_provider(host_data: HostData) -> NatsMessagingProvider {
    if let Some(c) = host_data.config_json.as_ref() {
        // empty string becomes the default configuration
        if c.trim().is_empty() {
            NatsMessagingProvider {
                host_id: host_data.host_id,
                ..Default::default()
            }
        } else {
            let config: ConnectionConfig = serde_json::from_str(c)
                .expect("JSON deserialization from connection config should have worked");
            NatsMessagingProvider {
                default_config: config,
                host_id: host_data.host_id,
                ..Default::default()
            }
        }
    } else {
        NatsMessagingProvider {
            host_id: host_data.host_id,
            ..Default::default()
        }
    }
}

/// Configuration for connecting a nats client.
/// More options are available if you use the json than variables in the values string map.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct ConnectionConfig {
    /// list of topics to subscribe to
    #[serde(default)]
    subscriptions: Vec<String>,
    #[serde(default)]
    cluster_uris: Vec<String>,
    #[serde(default)]
    auth_jwt: Option<String>,
    #[serde(default)]
    auth_seed: Option<String>,
    #[serde(default)]
    tls_ca: Option<String>,
    #[serde(default)]
    tls_ca_file: Option<String>,

    /// ping interval in seconds
    #[serde(default)]
    ping_interval_sec: Option<u16>,
}

impl ConnectionConfig {
    fn merge(&self, extra: &ConnectionConfig) -> ConnectionConfig {
        let mut out = self.clone();
        if !extra.subscriptions.is_empty() {
            out.subscriptions = extra.subscriptions.clone();
        }
        // If the default configuration has a URL in it, and then the link definition
        // also provides a URL, the assumption is to replace/override rather than combine
        // the two into a potentially incompatible set of URIs
        if !extra.cluster_uris.is_empty() {
            out.cluster_uris = extra.cluster_uris.clone();
        }
        if extra.auth_jwt.is_some() {
            out.auth_jwt = extra.auth_jwt.clone()
        }
        if extra.auth_seed.is_some() {
            out.auth_seed = extra.auth_seed.clone()
        }
        if extra.ping_interval_sec.is_some() {
            out.ping_interval_sec = extra.ping_interval_sec
        }
        if extra.tls_ca.is_some() {
            out.tls_ca = extra.tls_ca.clone()
        }
        if extra.tls_ca_file.is_some() {
            out.tls_ca_file = extra.tls_ca_file.clone()
        }
        out
    }
}

impl Default for ConnectionConfig {
    fn default() -> ConnectionConfig {
        ConnectionConfig {
            subscriptions: vec![],
            cluster_uris: vec![DEFAULT_NATS_URI.to_string()],
            auth_jwt: None,
            auth_seed: None,
            ping_interval_sec: None,
            tls_ca: None,
            tls_ca_file: None,
        }
    }
}

impl ConnectionConfig {
    fn new_from(values: &HashMap<String, String>) -> RpcResult<ConnectionConfig> {
        let mut config = if let Some(config_b64) = values.get("config_b64") {
            let bytes = base64::decode(config_b64.as_bytes()).map_err(|e| {
                RpcError::InvalidParameter(format!("invalid base64 encoding: {}", e))
            })?;
            serde_json::from_slice::<ConnectionConfig>(&bytes)
                .map_err(|e| RpcError::InvalidParameter(format!("corrupt config_b64: {}", e)))?
        } else if let Some(config) = values.get("config_json") {
            serde_json::from_str::<ConnectionConfig>(config)
                .map_err(|e| RpcError::InvalidParameter(format!("corrupt config_json: {}", e)))?
        } else {
            ConnectionConfig::default()
        };

        if let Some(sub) = values.get(ENV_NATS_SUBSCRIPTION) {
            config
                .subscriptions
                .extend(sub.split(',').map(|s| s.to_string()));
        }
        if let Some(url) = values.get(ENV_NATS_URI) {
            config.cluster_uris = url.split(',').map(String::from).collect();
        }
        if let Some(jwt) = values.get(ENV_NATS_CLIENT_JWT) {
            config.auth_jwt = Some(jwt.clone());
        }
        if let Some(seed) = values.get(ENV_NATS_CLIENT_SEED) {
            config.auth_seed = Some(seed.clone());
        }
        if config.auth_jwt.is_some() && config.auth_seed.is_none() {
            return Err(RpcError::InvalidParameter(
                "if you specify jwt, you must also specify a seed".to_string(),
            ));
        }
        if config.cluster_uris.is_empty() {
            config.cluster_uris.push(DEFAULT_NATS_URI.to_string());
        }
        if let Some(tls_ca) = values.get(ENV_NATS_TLS_CA) {
            config.tls_ca = Some(tls_ca.clone());
        }
        if let Some(tls_ca_file) = values.get(ENV_NATS_TLS_CA_FILE) {
            config.tls_ca_file = Some(tls_ca_file.clone());
        }
        Ok(config)
    }
}

/// NatsClientBundles hold a NATS client and information (subscriptions)
/// related to it.
///
/// This struct is necssary because subscriptions are *not* automatically removed on client drop,
/// meaning that we must keep track of all subscriptions to close once the client is done
#[derive(Debug)]
struct NatsClientBundle {
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

/// Nats implementation for wasmcloud:messaging
#[derive(Default, Clone, Provider)]
#[services(Messaging)]
struct NatsMessagingProvider {
    // store nats connection client per actor
    actors: Arc<RwLock<HashMap<String, NatsClientBundle>>>,
    default_config: ConnectionConfig,
    host_id: String,
}

// use default implementations of provider message handlers
impl ProviderDispatch for NatsMessagingProvider {}

impl NatsMessagingProvider {
    /// Attempt to connect to nats url (with jwt credentials, if provided)
    async fn connect(
        &self,
        cfg: ConnectionConfig,
        ld: &LinkDefinition,
    ) -> Result<NatsClientBundle, RpcError> {
        let opts = build_connect_options(&cfg)?;
        // Use the first visible cluster_uri
        let url = cfg.cluster_uris.first().unwrap();

        let client_name = format!(
            "NATS Messaging Provider - {} - {} - {}",
            self.host_id, ld.actor_id, ld.link_name
        );
        let client = wasmbus_rpc::rpc_client::with_connection_event_logging(opts.name(client_name)) // allow this to show up uniquely in a NATS connection list
            .connect(url)
            .await
            .map_err(|e| RpcError::ProviderInit(format!("NATS connection to {}: {}", url, e)))?;

        // Connections
        let mut sub_handles = Vec::new();
        for sub in cfg.subscriptions.iter().filter(|s| !s.is_empty()) {
            let (sub, queue) = match sub.split_once('|') {
                Some((sub, queue)) => (sub, Some(queue.to_string())),
                None => (sub.as_str(), None),
            };

            sub_handles.push((
                sub.to_string(),
                self.subscribe(&client, ld, sub.to_string(), queue).await?,
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
        ld: &LinkDefinition,
        sub: String,
        queue: Option<String>,
    ) -> RpcResult<JoinHandle<()>> {
        let mut subscriber = match queue {
            Some(queue) => client.queue_subscribe(sub.clone(), queue).await,
            None => client.subscribe(sub.clone()).await,
        }
        .map_err(|e| {
            error!(subject = %sub, error = %e, "error subscribing subscribing");
            RpcError::Nats(format!("subscription to {}: {}", sub, e))
        })?;

        let link_def = ld.to_owned();

        // Spawn a thread that listens for messages coming from NATS
        // this thread is expected to run the full duration that the provider is available
        let join_handle = tokio::spawn(async move {
            // MAGIC NUMBER: Based on our benchmark testing, this seems to be a good upper limit
            // where we start to get diminishing returns. We can consider making this
            // configurable down the line.
            // NOTE (thomastaylor312): It may be better to have a semaphore pool on the
            // NatsMessagingProvider struct that has a global limit of permits so that we don't end
            // up with 20 subscriptions all getting slammed with up to 75 tasks, but we should wait
            // to do anything until we see what happens with real world usage and benchmarking
            let semaphore = Arc::new(Semaphore::new(75));

            // Listen for NATS message(s)
            while let Some(msg) = subscriber.next().await {
                // Set up tracing context for the NATS message
                let span = tracing::debug_span!("handle_message", actor_id = %link_def.actor_id);
                span.in_scope(|| {
                    wasmbus_rpc::otel::attach_span_context(&msg);
                });

                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => {
                        warn!("Work pool has been closed, exiting queue subscribe");
                        break;
                    }
                };

                tokio::spawn(dispatch_msg(link_def.clone(), msg, permit).instrument(span));
            }

            // The NATS subscriber stream should never close
            error!(topic = %sub, "FATAL: NATS subscriber stream closed unexpectedly");
        });

        Ok(join_handle)
    }
}

#[instrument(level = "debug", skip_all, fields(actor_id = %link_def.actor_id, subject = %nats_msg.subject, reply_to = ?nats_msg.reply))]
async fn dispatch_msg(
    link_def: LinkDefinition,
    nats_msg: async_nats::Message,
    _permit: OwnedSemaphorePermit,
) {
    let msg = SubMessage {
        body: nats_msg.payload.into(),
        reply_to: nats_msg.reply,
        subject: nats_msg.subject,
    };
    let actor = MessageSubscriberSender::for_actor(&link_def);
    if let Err(e) = actor.handle_message(&Context::default(), &msg).await {
        error!(
            error = %e,
            "Unable to send subscription"
        );
    }
}

/// Handle provider control commands
/// put_link (new actor link command), del_link (remove link command), and shutdown
#[async_trait]
impl ProviderHandler for NatsMessagingProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip(self, ld), fields(actor_id = %ld.actor_id))]
    async fn put_link(&self, ld: &LinkDefinition) -> RpcResult<bool> {
        // If the link definition values are empty, use the default connection configuration
        let config = if ld.values.is_empty() {
            self.default_config.clone()
        } else {
            // create a config from the supplied values and merge that with the existing default
            match ConnectionConfig::new_from(&ld.values) {
                Ok(cc) => self.default_config.merge(&cc),
                Err(e) => {
                    error!("Failed to build connection configuration: {e:?}");
                    return Ok(false);
                }
            }
        };

        let mut update_map = self.actors.write().await;
        update_map.insert(ld.actor_id.to_string(), self.connect(config, ld).await?);

        Ok(true)
    }

    /// Handle notification that a link is dropped: close the connection
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, actor_id: &str) {
        let mut aw = self.actors.write().await;

        if let Some(bundle) = aw.remove(actor_id) {
            // Note: subscriptions will be closed via Drop on the NatsClientBundle
            debug!(
                "closing [{}] NATS subscriptions for actor [{}]...",
                &bundle.sub_handles.len(),
                actor_id,
            );
        }

        debug!("finished processing delete link for actor [{}]", actor_id);
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> Result<(), Infallible> {
        let mut aw = self.actors.write().await;
        // empty the actor link data and stop all servers
        aw.clear();
        // dropping all connections should send unsubscribes and close the connections.
        Ok(())
    }
}

/// Handle Messaging methods that interact with redis
#[async_trait]
impl Messaging for NatsMessagingProvider {
    #[instrument(level = "debug", skip(self, ctx, msg), fields(actor_id = ?ctx.actor, subject = %msg.subject, reply_to = ?msg.reply_to, body_len = %msg.body.len()))]
    async fn publish(&self, ctx: &Context, msg: &PubMessage) -> RpcResult<()> {
        let actor_id = ctx
            .actor
            .as_ref()
            .ok_or_else(|| RpcError::InvalidParameter("no actor in request".to_string()))?;

        // get read lock on actor-client hashmap to get the connection, then drop it
        let _rd = self.actors.read().await;

        let nats_bundle = _rd
            .get(actor_id)
            .ok_or_else(|| RpcError::InvalidParameter(format!("actor not linked:{}", actor_id)))?;
        let nats_client = nats_bundle.client.clone();
        drop(_rd);

        let headers = OtelHeaderInjector::default_with_span().into();

        let res = match msg.reply_to.clone() {
            Some(reply_to) => if should_strip_headers(&msg.subject) {
                nats_client
                    .publish_with_reply(msg.subject.to_string(), reply_to, msg.body.clone().into())
                    .await
            } else {
                nats_client
                    .publish_with_reply_and_headers(
                        msg.subject.to_string(),
                        reply_to,
                        headers,
                        msg.body.clone().into(),
                    )
                    .await
            }
            .map_err(|e| RpcError::Nats(e.to_string())),
            None => nats_client
                .publish_with_headers(msg.subject.to_string(), headers, msg.body.clone().into())
                .await
                .map_err(|e| RpcError::Nats(e.to_string())),
        };
        let _ = nats_client.flush().await;
        res
    }

    #[instrument(level = "debug", skip(self, ctx, msg), fields(actor_id = ?ctx.actor, subject = %msg.subject))]
    async fn request(&self, ctx: &Context, msg: &RequestMessage) -> RpcResult<ReplyMessage> {
        let actor_id = ctx
            .actor
            .as_ref()
            .ok_or_else(|| RpcError::InvalidParameter("no actor in request".to_string()))?;
        // Obtain read lock on actor-client hashmap
        let _rd = self.actors.read().await;

        // Extract NATS client from bundle
        let nats_client_bundle = _rd
            .get(actor_id)
            .ok_or_else(|| RpcError::InvalidParameter(format!("actor not linked:{}", actor_id)))?;
        let nats_client = nats_client_bundle.client.clone();
        drop(_rd); // early release of actor-client map

        // Inject OTEL headers
        let headers = OtelHeaderInjector::default_with_span().into();

        // Perform the request with a timeout
        let request_with_timeout = if should_strip_headers(&msg.subject) {
            tokio::time::timeout(
                Duration::from_millis(msg.timeout_ms as u64),
                nats_client.request(msg.subject.to_string(), msg.body.clone().into()),
            )
            .await
        } else {
            tokio::time::timeout(
                Duration::from_millis(msg.timeout_ms as u64),
                nats_client.request_with_headers(
                    msg.subject.to_string(),
                    headers,
                    msg.body.clone().into(),
                ),
            )
            .await
        };

        // Process results of request
        match request_with_timeout {
            Err(_timeout_err) => Err(RpcError::Timeout("nats request timed out".to_string())),
            Ok(Err(send_err)) => Err(RpcError::Nats(format!("nats send error: {}", send_err))),
            Ok(Ok(resp)) => Ok(ReplyMessage {
                body: resp.payload.to_vec(),
                reply_to: resp.reply,
                subject: resp.subject,
            }),
        }
    }
}

// In the current version of the NATS server, using headers on certain $SYS.REQ topics will cause server-side
// parse failures
fn should_strip_headers(topic: &str) -> bool {
    topic.starts_with("$SYS")
}

fn build_connect_options(cfg: &ConnectionConfig) -> Result<async_nats::ConnectOptions, RpcError> {
    let opts = match (cfg.auth_jwt.clone(), cfg.auth_seed.clone()) {
        (Some(jwt), Some(seed)) => {
            let key_pair = std::sync::Arc::new(
                KeyPair::from_seed(&seed)
                    .map_err(|e| RpcError::ProviderInit(format!("key init: {}", e)))?,
            );
            async_nats::ConnectOptions::with_jwt(jwt, move |nonce| {
                let key_pair = key_pair.clone();
                async move { key_pair.sign(&nonce).map_err(async_nats::AuthError::new) }
            })
        }
        (None, None) => async_nats::ConnectOptions::default(),
        _ => {
            return Err(RpcError::InvalidParameter(
                "must provide both jwt and seed for jwt authentication".into(),
            ));
        }
    };

    if let Some(tls_ca) = &cfg.tls_ca {
        return add_tls_ca(tls_ca, opts);
    } else if let Some(tls_ca_file) = &cfg.tls_ca_file {
        let ca = std::fs::read_to_string(tls_ca_file)
            .map_err(|e| RpcError::ProviderInit(format!("tls ca file: {}", e)))?;
        return add_tls_ca(&ca, opts);
    }

    Ok(opts)
}

fn add_tls_ca(
    tls_ca: &str,
    opts: async_nats::ConnectOptions,
) -> Result<async_nats::ConnectOptions, RpcError> {
    let mut store_builder = async_nats::rustls::RootCertStore::empty();
    let parsed_ca = rustls_pemfile::read_one(&mut tls_ca.as_bytes())
        .map_err(|e| RpcError::ProviderInit(format!("tls ca: {}", e)))?;

    if let Some(rustls_pemfile::Item::X509Certificate(cert)) = parsed_ca {
        store_builder.add_parsable_certificates(&[cert]);
    } else {
        return Err(RpcError::ProviderInit(
            "tls ca: invalid certificate type, must be a DER encoded PEM file".into(),
        ));
    };

    let tls_client = async_nats::rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(store_builder)
        .with_no_client_auth();
    Ok(opts.tls_client_config(tls_client).require_tls(true))
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use crate::{generate_provider, ConnectionConfig, NatsMessagingProvider};
    use wasmbus_rpc::{
        core::{HostData, LinkDefinition},
        error::RpcError,
        provider::ProviderHandler,
    };

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
        assert_eq!(config.auth_jwt.unwrap(), "authy");
        assert_eq!(config.auth_seed.unwrap(), "seedy");
        assert_eq!(config.cluster_uris, ["nats://soyvuh"]);
        assert!(config.subscriptions.is_empty());
        assert!(config.ping_interval_sec.is_none());
    }

    #[test]
    fn test_generate_provider_works_with_empty_string() {
        let mut host_data = HostData::default();
        host_data.config_json = Some("".to_string());
        let prov = generate_provider(host_data);
        assert_eq!(prov.default_config, ConnectionConfig::default());
    }

    #[test]
    fn test_generate_provider_works_with_none() {
        let mut host_data = HostData::default();
        host_data.config_json = None;
        let prov = generate_provider(host_data);
        assert_eq!(prov.default_config, ConnectionConfig::default());
    }

    #[test]
    fn test_connectionconfig_merge() {
        // second > original, individual vec fields are replace not extend
        let mut cc1 = ConnectionConfig::default();
        cc1.cluster_uris = vec!["old_server".to_string()];
        cc1.subscriptions = vec!["topic1".to_string()];
        let mut cc2 = ConnectionConfig::default();
        cc2.cluster_uris = vec!["server1".to_string(), "server2".to_string()];
        cc2.auth_jwt = Some("jawty".to_string());
        let cc3 = cc1.merge(&cc2);
        assert_eq!(cc3.cluster_uris, cc2.cluster_uris);
        assert_eq!(cc3.subscriptions, cc1.subscriptions);
        assert_eq!(cc3.auth_jwt, Some("jawty".to_string()))
    }

    /// Ensure that unlink triggers subscription removal
    /// https://github.com/wasmCloud/capability-providers/issues/196
    ///
    /// NOTE: this is tested here for easy access to put_link/del_link without
    /// the fuss of loading/managing individual actors in the lattice
    #[tokio::test]
    async fn test_link_unsub() -> anyhow::Result<()> {
        // Build a nats messaging provider
        let prov = NatsMessagingProvider::default();

        // Actor should have no clients and no subs before hand
        let actor_map = prov.actors.write().await;
        assert_eq!(actor_map.len(), 0);
        drop(actor_map);

        // Add a provider
        let mut ld = LinkDefinition::default();
        ld.actor_id = String::from("???");
        ld.link_name = String::from("test");
        ld.contract_id = String::from("test");
        ld.values = HashMap::<String, String>::from([
            (
                String::from("SUBSCRIPTION"),
                String::from("test.wasmcloud.unlink"),
            ),
            (String::from("URI"), String::from("127.0.0.1:4222")),
        ]);
        prov.put_link(&ld).await?;

        // After putting a link there should be one sub
        let actor_map = prov.actors.write().await;
        assert_eq!(actor_map.len(), 1);
        assert_eq!(actor_map.get("???").unwrap().sub_handles.len(), 1);
        drop(actor_map);

        // Remove link (this should kill the subscription)
        let _ = prov.delete_link(&ld.actor_id).await;

        // After removing a link there should be no subs
        let actor_map = prov.actors.write().await;
        assert_eq!(actor_map.len(), 0);
        drop(actor_map);

        let _ = prov.shutdown().await;
        Ok(())
    }

    /// Ensure that provided URIs are honored by NATS provider
    /// https://github.com/wasmCloud/capability-providers/issues/231
    ///
    /// NOTE: This test can't be rolled into the put_link test because
    /// NATS does not store the URL you fed it to connect -- it stores the host's view in
    /// [async_nats::ServerInfo]
    #[tokio::test]
    async fn test_link_value_uri_usage() -> anyhow::Result<()> {
        // Build a nats messaging provider
        let prov = NatsMessagingProvider::default();

        // Actor should have no clients and no subs before hand
        let actor_map = prov.actors.write().await;
        assert_eq!(actor_map.len(), 0);
        drop(actor_map);

        // Add a provider
        let mut ld = LinkDefinition::default();
        ld.actor_id = String::from("???");
        ld.link_name = String::from("test");
        ld.contract_id = String::from("test");
        ld.values = HashMap::<String, String>::from([
            (
                String::from("SUBSCRIPTION"),
                String::from("test.wasmcloud.unlink"),
            ),
            (String::from("URI"), String::from("99.99.99.99:4222")),
        ]);
        let result = prov.put_link(&ld).await;

        // Expect the result to fail, connecting to an IP that (should) not exist
        assert!(result.is_err(), "put_link failed");
        assert!(
            matches!(result, Err(RpcError::ProviderInit(msg)) if msg == "NATS connection to 99.99.99.99:4222: timed out")
        );

        let _ = prov.shutdown().await;
        Ok(())
    }
}
