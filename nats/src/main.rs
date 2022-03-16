//! Nats implementation for wasmcloud:messaging.
//!
use std::{collections::HashMap, convert::Infallible, sync::Arc};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{error, info, instrument};
use tracing_futures::Instrument;
use wascap::prelude::KeyPair;
use wasmbus_rpc::{anats, core::LinkDefinition, provider::prelude::*};
use wasmcloud_interface_messaging::{
    MessageSubscriber, MessageSubscriberSender, Messaging, MessagingReceiver, PubMessage,
    ReplyMessage, RequestMessage, SubMessage,
};

const DEFAULT_NATS_URI: &str = "0.0.0.0:4222";
const ENV_NATS_SUBSCRIPTION: &str = "SUBSCRIPTION";
const ENV_NATS_URI: &str = "URI";
const ENV_NATS_CLIENT_JWT: &str = "CLIENT_JWT";
const ENV_NATS_CLIENT_SEED: &str = "CLIENT_SEED";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(atty::is(atty::Stream::Stderr))
        .init();
    // handle lattice control messages and forward rpc to the provider dispatch
    // returns when provider receives a shutdown control message
    provider_main(NatsMessagingProvider::default())?;

    eprintln!("Nats-messaging provider exiting");
    Ok(())
}

/// Configuration for connecting a nats client.
/// More options are available if you use the json than variables in the values string map.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

    /// ping interval in seconds
    #[serde(default)]
    ping_interval_sec: Option<u16>,
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
            config.cluster_uris.push(url.clone());
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
        Ok(config)
    }
}

/// Nats implementation for wasmcloud:messaging
#[derive(Default, Clone, Provider)]
#[services(Messaging)]
struct NatsMessagingProvider {
    // store nats connection client per actor
    actors: Arc<RwLock<HashMap<String, anats::Connection>>>,
}
// use default implementations of provider message handlers
impl ProviderDispatch for NatsMessagingProvider {}

impl NatsMessagingProvider {
    /// attempt to connect to nats url (with jwt credentials, if provided)
    async fn connect(
        &self,
        cfg: ConnectionConfig,
        ld: &LinkDefinition,
    ) -> Result<anats::Connection, RpcError> {
        let mut opts = match (cfg.auth_jwt, cfg.auth_seed) {
            (Some(jwt), Some(seed)) => {
                let kp = KeyPair::from_seed(&seed)
                    .map_err(|e| RpcError::ProviderInit(format!("key init: {}", e)))?;
                anats::Options::with_jwt(
                    move || Ok(jwt.clone()),
                    move |nonce| kp.sign(nonce).unwrap(),
                )
            }
            (None, None) => anats::Options::new(),
            _ => {
                return Err(RpcError::InvalidParameter(
                    "must provide both jwt and seed for jwt authentication".into(),
                ));
            }
        };
        opts = opts.with_name("wasmCloud nats-messaging provider");
        let url = cfg.cluster_uris.get(0).unwrap();
        let conn = opts
            .connect(url)
            .await
            .map_err(|e| RpcError::ProviderInit(format!("Nats connection to {}: {}", url, e)))?;

        for sub in cfg.subscriptions.iter().filter(|s| !s.is_empty()) {
            let (sub, queue) = match sub.split_once('|') {
                Some((sub, queue)) => (sub, Some(queue)),
                None => (sub.as_str(), None),
            };
            self.subscribe(&conn, ld, sub, queue).await?;
        }
        Ok(conn)
    }

    /// send message to subscriber
    #[instrument(level = "debug", skip(self, ld, nats_msg), fields(actor_id = %ld.actor_id, subject = %nats_msg.subject, reply_to = ?nats_msg.reply))]
    async fn dispatch_msg(&self, ld: &LinkDefinition, nats_msg: anats::Message) {
        let msg = SubMessage {
            body: nats_msg.data,
            reply_to: nats_msg.reply,
            subject: nats_msg.subject,
        };
        let actor = MessageSubscriberSender::for_actor(ld);
        if let Err(e) = actor.handle_message(&Context::default(), &msg).await {
            error!(
                error = %e,
                "Unable to send subscription"
            );
        }
    }

    /// Add a regular or queue subscription
    async fn subscribe(
        &self,
        conn: &anats::Connection,
        ld: &LinkDefinition,
        sub: &str,
        queue: Option<&str>,
    ) -> RpcResult<()> {
        let subscription = match queue {
            Some(queue) => conn.queue_subscribe(sub, queue).await,
            None => conn.subscribe(sub).await,
        }
        .map_err(|e| {
            error!(subject = %sub, error = %e, "error subscribing subscribing");
            RpcError::Nats(format!("subscription to {}: {}", sub, e))
        })?;
        let this = self.clone();
        let link_def = ld.clone();
        let _join_handle = tokio::spawn(
            async move {
                while let Some(msg) = subscription.next().await {
                    this.dispatch_msg(&link_def, msg).await;
                }
            }
            .instrument(
                tracing::debug_span!("subscription", actor_id = %ld.actor_id, subject = %sub),
            ),
        );
        Ok(())
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
        let config = ConnectionConfig::new_from(&ld.values)?;
        let conn = self.connect(config, ld).await?;

        let mut update_map = self.actors.write().await;
        update_map.insert(ld.actor_id.to_string(), conn);

        Ok(true)
    }

    /// Handle notification that a link is dropped: close the connection
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, actor_id: &str) {
        let mut aw = self.actors.write().await;
        if let Some(conn) = aw.remove(actor_id) {
            info!("nats closing connection for actor {}", actor_id);
            // close and drop the connection
            let _ = conn.close().await;
        }
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> Result<(), Infallible> {
        let mut aw = self.actors.write().await;
        // empty the actor link data and stop all servers
        for (_, conn) in aw.drain() {
            // close and drop each connection
            let _ = conn.close().await;
        }
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
        // get read lock on actor-client hashmap
        let rd = self.actors.read().await;
        let conn = rd
            .get(actor_id)
            .ok_or_else(|| RpcError::InvalidParameter(format!("actor not linked:{}", actor_id)))?;
        match &msg.reply_to {
            Some(reply_to) => {
                conn.publish_request(&msg.subject, reply_to, &msg.body)
                    .await
            }
            None => conn.publish(&msg.subject, &msg.body).await,
        }
        .map_err(|e| RpcError::Nats(e.to_string()))?;
        Ok(())
    }

    #[instrument(level = "debug", skip(self, ctx, msg), fields(actor_id = ?ctx.actor, subject = %msg.subject))]
    async fn request(&self, ctx: &Context, msg: &RequestMessage) -> RpcResult<ReplyMessage> {
        let actor_id = ctx
            .actor
            .as_ref()
            .ok_or_else(|| RpcError::InvalidParameter("no actor in request".to_string()))?;
        // get read lock on actor-client hashmap
        let rd = self.actors.read().await;
        let conn = rd
            .get(actor_id)
            .ok_or_else(|| RpcError::InvalidParameter(format!("actor not linked:{}", actor_id)))?;
        let resp = conn
            .request_timeout(
                &msg.subject,
                &msg.body,
                std::time::Duration::from_millis(msg.timeout_ms as u64),
            )
            .await
            .map_err(|e| RpcError::Nats(e.to_string()))?;
        Ok(ReplyMessage {
            body: resp.data,
            reply_to: resp.reply,
            subject: resp.subject,
        })
    }
}
