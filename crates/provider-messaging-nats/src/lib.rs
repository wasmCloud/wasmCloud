use core::time::Duration;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _};
use futures::StreamExt;
use opentelemetry_nats::{attach_span_context, NatsHeaderInjector};
use tokio::fs;
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, error, instrument, warn};
use tracing_futures::Instrument;
use wascap::prelude::KeyPair;

use wasmcloud_provider_wit_bindgen::deps::{
    async_trait::async_trait,
    wasmcloud_provider_sdk::core::{ComponentId, HostData},
    wasmcloud_provider_sdk::{Context, LinkConfig, ProviderOperationResult},
};

mod connection;
use connection::ConnectionConfig;

wasmcloud_provider_wit_bindgen::generate!({
    impl_struct: NatsMessagingProvider,
    contract: "wasmcloud:messaging",
    wit_bindgen_cfg: "provider-messaging-nats"
});

/// [`NatsClientBundle`]s hold a NATS client and information (subscriptions)
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
#[derive(Default, Clone)]
pub struct NatsMessagingProvider {
    // store nats connection client per actor
    actors: Arc<RwLock<HashMap<String, NatsClientBundle>>>,
    default_config: ConnectionConfig,
}

impl NatsMessagingProvider {
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
        component_id: ComponentId,
    ) -> anyhow::Result<NatsClientBundle> {
        let mut opts = match (cfg.auth_jwt, cfg.auth_seed) {
            (Some(jwt), Some(seed)) => {
                let seed = KeyPair::from_seed(&seed).context("failed to parse seed key pair")?;
                let seed = Arc::new(seed);
                async_nats::ConnectOptions::with_jwt(jwt, move |nonce| {
                    let seed = seed.clone();
                    async move { seed.sign(&nonce).map_err(async_nats::AuthError::new) }
                })
            }
            (None, None) => async_nats::ConnectOptions::default(),
            _ => bail!("must provide both jwt and seed for jwt authentication"),
        };
        if let Some(tls_ca) = &cfg.tls_ca {
            opts = add_tls_ca(tls_ca, opts)?;
        } else if let Some(tls_ca_file) = &cfg.tls_ca_file {
            let ca = fs::read_to_string(tls_ca_file)
                .await
                .context("failed to read TLS CA file")?;
            opts = add_tls_ca(&ca, opts)?;
        }

        // Use the first visible cluster_uri
        let url = cfg.cluster_uris.first().unwrap();

        let client = opts
            .name("NATS Messaging Provider") // allow this to show up uniquely in a NATS connection list
            .connect(url)
            .await?;

        // Connections
        let mut sub_handles = Vec::new();
        for sub in cfg.subscriptions.iter().filter(|s| !s.is_empty()) {
            let (sub, queue) = match sub.split_once('|') {
                Some((sub, queue)) => (sub, Some(queue.to_string())),
                None => (sub.as_str(), None),
            };

            sub_handles.push((
                sub.to_string(),
                self.subscribe(&client, component_id.clone(), sub.to_string(), queue)
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
        component_id: ComponentId,
        sub: String,
        queue: Option<String>,
    ) -> anyhow::Result<JoinHandle<()>> {
        let mut subscriber = match queue {
            Some(queue) => client.queue_subscribe(sub.clone(), queue).await,
            None => client.subscribe(sub.clone()).await,
        }?;

        debug!(?component_id, "spawning listener for component");

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
                let component_id = component_id.clone();
                debug!(?msg, ?component_id, "received messsage");
                // Set up tracing context for the NATS message
                let span = tracing::debug_span!("handle_message", component_id);

                span.in_scope(|| {
                    attach_span_context(&msg);
                });

                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => {
                        warn!("Work pool has been closed, exiting queue subscribe");
                        break;
                    }
                };

                tokio::spawn(dispatch_msg(component_id, msg, permit).instrument(span));
            }
        });

        Ok(join_handle)
    }
}

#[instrument(level = "debug", skip_all, fields(component_id = %component_id, subject = %nats_msg.subject, reply_to = ?nats_msg.reply))]
async fn dispatch_msg(
    component_id: ComponentId,
    nats_msg: async_nats::Message,
    _permit: OwnedSemaphorePermit,
) {
    let msg = BrokerMessage {
        body: nats_msg.payload.into(),
        reply_to: nats_msg.reply.map(|s| s.to_string()),
        subject: nats_msg.subject.to_string(),
    };
    let actor = InvocationHandler::new(component_id.clone());
    debug!(
        subject = msg.subject,
        reply_to = ?msg.reply_to,
        component_id = component_id,
        "sending message to actor",
    );
    if let Err(e) = actor.handle_message(msg).await {
        error!(
            error = %e,
            "Unable to send message"
        );
    }
}

/// Handle provider control commands
/// put_link (new actor link command), del_link (remove link command), and shutdown
#[async_trait]
impl WasmcloudCapabilityProvider for NatsMessagingProvider {
    /// Provider should perform any operations needed for a new link,
    /// including setting up per-actor resources, and checking authorization.
    /// If the link is allowed, return true, otherwise return false to deny the link.
    #[instrument(level = "debug", skip_all, fields(source_id = %link_config.get_source_id()))]
    async fn receive_link_config_as_target(
        &self,
        link_config: impl LinkConfig,
    ) -> ProviderOperationResult<()> {
        let source_id = link_config.get_source_id();
        let config_values = link_config.get_config();
        let config = if config_values.is_empty() {
            self.default_config.clone()
        } else {
            // create a config from the supplied values and merge that with the existing default
            match ConnectionConfig::from_map(config_values) {
                Ok(cc) => self.default_config.merge(&cc),
                Err(e) => {
                    error!("Failed to build connection configuration: {e:?}");
                    return Err(anyhow!(e)
                        .context("failed to build connection config")
                        .into());
                }
            }
        };

        let mut update_map = self.actors.write().await;
        let bundle = match self.connect(config, source_id.into()).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to connect to NATS: {e:?}");
                return Err(anyhow!(e).context("failed to connect to NATS").into());
            }
        };
        update_map.insert(source_id.into(), bundle);

        Ok(())
    }

    /// Handle notification that a link is dropped: close the connection
    #[instrument(level = "info", skip(self))]
    async fn delete_link(&self, source_id: &str) -> ProviderOperationResult<()> {
        let mut aw = self.actors.write().await;

        if let Some(bundle) = aw.remove(source_id) {
            // Note: subscriptions will be closed via Drop on the NatsClientBundle
            debug!(
                "closing [{}] NATS subscriptions for actor [{}]...",
                &bundle.sub_handles.len(),
                source_id,
            );
        }

        debug!("finished processing delete link for actor [{}]", source_id);
        Ok(())
    }

    /// Handle shutdown request by closing all connections
    async fn shutdown(&self) -> ProviderOperationResult<()> {
        let mut aw = self.actors.write().await;
        // empty the actor link data and stop all servers
        aw.clear();
        // dropping all connections should send unsubscribes and close the connections, so no need
        // to handle that here
        Ok(())
    }
}

/// Implement the 'wasmcloud:messaging' capability provider interface
#[async_trait]
impl WasmcloudMessagingConsumer for NatsMessagingProvider {
    #[instrument(level = "debug", skip(self, ctx, msg), fields(source_id = ?ctx.actor, subject = %msg.subject, reply_to = ?msg.reply_to, body_len = %msg.body.len()))]
    async fn publish(&self, ctx: Context, msg: BrokerMessage) -> Result<(), String> {
        let source_id = match ctx.actor.as_ref() {
            Some(source_id) => source_id,
            None => {
                error!("no actor in request");
                return Err("no actor in request".to_string());
            }
        };

        // get read lock on actor-client hashmap to get the connection, then drop it
        let _rd = self.actors.read().await;

        let nats_client = {
            let rd = self.actors.read().await;
            let nats_bundle = match rd.get(source_id) {
                Some(nats_bundle) => nats_bundle,
                None => {
                    error!("actor not linked: {source_id}");
                    return Err(format!("actor not linked: {source_id}"));
                }
            };
            nats_bundle.client.clone()
        };

        let headers = NatsHeaderInjector::default_with_span().into();

        let body = msg.body.into();
        let res = match msg.reply_to.clone() {
            Some(reply_to) => if should_strip_headers(&msg.subject) {
                nats_client
                    .publish_with_reply(msg.subject.to_string(), reply_to, body)
                    .await
            } else {
                nats_client
                    .publish_with_reply_and_headers(
                        msg.subject.to_string(),
                        reply_to,
                        headers,
                        body,
                    )
                    .await
            }
            .map_err(|e| e.to_string()),
            None => nats_client
                .publish_with_headers(msg.subject.to_string(), headers, body)
                .await
                .map_err(|e| e.to_string()),
        };
        let _ = nats_client.flush().await;
        res
    }

    #[instrument(level = "debug", skip(self, ctx), fields(source_id = ?ctx.actor, subject = %subject))]
    async fn request(
        &self,
        ctx: Context,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> Result<BrokerMessage, String> {
        let source_id = match ctx.actor.as_ref() {
            Some(source_id) => source_id,
            None => {
                error!("no actor in request");
                return Err("no actor in request".to_string());
            }
        };

        let nats_client = {
            let rd = self.actors.read().await;
            let nats_bundle = match rd.get(source_id) {
                Some(nats_bundle) => nats_bundle,
                None => {
                    error!("actor not linked: {source_id}");
                    return Err(format!("actor not linked: {source_id}"));
                }
            };
            nats_bundle.client.clone()
        }; // early release of actor-client map

        // Inject OTEL headers
        let headers = NatsHeaderInjector::default_with_span().into();

        let timeout = Duration::from_millis(timeout_ms.into());
        let body = body.into();
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
                return Err(format!("nats request timed out: {timeout_err}"));
            }
            Ok(Err(send_err)) => {
                error!("nats send error: {send_err}");
                return Err(format!("nats send error: {send_err}"));
            }
            Ok(Ok(resp)) => Ok(BrokerMessage {
                body: resp.payload.to_vec(),
                reply_to: resp.reply.map(|s| s.to_string()),
                subject: resp.subject.to_string(),
            }),
        }
    }
}

// In the current version of the NATS server, using headers on certain $SYS.REQ topics will cause server-side
// parse failures
fn should_strip_headers(topic: &str) -> bool {
    topic.starts_with("$SYS")
}

fn add_tls_ca(
    tls_ca: &str,
    opts: async_nats::ConnectOptions,
) -> anyhow::Result<async_nats::ConnectOptions> {
    let ca = rustls_pemfile::read_one(&mut tls_ca.as_bytes()).context("failed to read CA")?;
    let mut roots = async_nats::rustls::RootCertStore::empty();
    if let Some(rustls_pemfile::Item::X509Certificate(ca)) = ca {
        roots.add_parsable_certificates(&[ca]);
    } else {
        bail!("tls ca: invalid certificate type, must be a DER encoded PEM file")
    };
    let tls_client = async_nats::rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(opts.tls_client_config(tls_client).require_tls(true))
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use wasmcloud_provider_wit_bindgen::deps::serde_json;
    use wasmcloud_provider_wit_bindgen::deps::wasmcloud_provider_sdk::ProviderHandler;

    use crate::{ConnectionConfig, NatsMessagingProvider};

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
    fn test_connectionconfig_merge() {
        // second > original, individual vec fields are replace not extend
        let cc1 = ConnectionConfig {
            cluster_uris: vec!["old_server".to_string()],
            subscriptions: vec!["topic1".to_string()],
            ..Default::default()
        };
        let cc2 = ConnectionConfig {
            cluster_uris: vec!["server1".to_string(), "server2".to_string()],
            auth_jwt: Some("jawty".to_string()),
            ..Default::default()
        };
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

        // Link the provider
        let link_config = (
            &String::from("some-source"),
            &String::from("some-provider"),
            &String::from("default"),
            &HashMap::new(),
        );
        assert!(prov
            .receive_link_config_as_target(link_config)
            .await
            .is_ok());

        // After putting a link there should be one sub
        let actor_map = prov.actors.write().await;
        assert_eq!(actor_map.len(), 1);
        assert_eq!(actor_map.get("???").unwrap().sub_handles.len(), 1);
        drop(actor_map);

        // Remove link (this should kill the subscription)
        let _ = prov.delete_link(link_config.0).await;

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
        let link_config = (
            &String::from("some-source"),
            &String::from("some-target"),
            &String::from("test"),
            &HashMap::from([
                ("SUBSCRIPTION".into(), "test.wasmcloud.unlink".into()),
                ("URI".into(), "99.99.99.99:4222".into()),
            ]),
        );
        assert!(prov
            .receive_link_config_as_target(link_config)
            .await
            .is_ok());
        assert!(prov.shutdown().await.is_ok());
        Ok(())
    }
}
