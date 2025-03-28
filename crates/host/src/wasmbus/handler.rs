use core::any::Any;
use core::iter::{repeat, zip};
use std::collections::{BTreeMap, HashMap};
use std::ops::Deref;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _};
use async_nats::header::{IntoHeaderName as _, IntoHeaderValue as _};
use async_trait::async_trait;
use bytes::Bytes;
use secrecy::SecretBox;
#[cfg(unix)]
use spire_api::{
    selectors::Selector, DelegateAttestationRequest::Selectors, DelegatedIdentityClient,
};
use tokio::sync::RwLock;
use tracing::{error, instrument, warn};
use wasmcloud_runtime::capability::logging::logging;
use wasmcloud_runtime::capability::secrets::store::SecretValue;
use wasmcloud_runtime::capability::{
    self, identity, messaging0_2_0, messaging0_3_0, secrets, CallTargetInterface,
};
use wasmcloud_runtime::component::{
    Bus, Bus1_0_0, Config, Error, Identity, InvocationErrorIntrospect, InvocationErrorKind,
    Logging, Messaging0_2, Messaging0_3, MessagingClient0_3, MessagingGuestMessage0_3,
    MessagingHostMessage0_3, ReplacedInstanceTarget, Secrets,
};
use wasmcloud_tracing::context::TraceContextInjector;
use wrpc_transport::InvokeExt as _;

use super::config::ConfigBundle;
use super::{injector_to_headers, Features};

// The key used to represent a wasmCloud-specific selector:
// https://github.com/spiffe/spire-api-sdk/blob/3c6b1447f3d82210b91462d003f6c2774ffbe472/proto/spire/api/types/selector.proto#L6-L8
//
// Similar to existing types defined in the spire-api crate: https://github.com/maxlambrecht/rust-spiffe/blob/929a090f99d458dd67fa499b74afbeb2fc44b114/spire-api/src/selectors.rs#L4-L5
const WASMCLOUD_SELECTOR_TYPE: &str = "wasmcloud";
// Similar to the existing Kubernetes types: https://github.com/maxlambrecht/rust-spiffe/blob/929a090f99d458dd67fa499b74afbeb2fc44b114/spire-api/src/selectors.rs#L38-L39
const WASMCLOUD_SELECTOR_COMPONENT: &str = "component";

#[derive(Clone, Debug)]
pub struct Handler {
    pub nats: Arc<async_nats::Client>,
    // ConfigBundle is perfectly safe to pass around, but in order to update it on the fly, we need
    // to have it behind a lock since it can be cloned and because the `Actor` struct this gets
    // placed into is also inside of an Arc
    pub config_data: Arc<RwLock<ConfigBundle>>,
    /// Secrets are cached per-[`Handler`] so they can be used at runtime without consulting the secrets
    /// backend for each request. The [`SecretValue`] is wrapped in the [`Secret`] type from the `secrecy`
    /// crate to ensure that it is not accidentally logged or exposed in error messages.
    pub secrets: Arc<RwLock<HashMap<String, SecretBox<SecretValue>>>>,
    /// The lattice this handler will use for RPC
    pub lattice: Arc<str>,
    /// The identifier of the component that this handler is associated with
    pub component_id: Arc<str>,
    /// The current link targets. `instance` -> `link-name`
    /// Instance specification does not include a version
    pub targets: Arc<RwLock<HashMap<Box<str>, Arc<str>>>>,

    /// Map of link names -> instance -> Target
    ///
    /// While a target may often be a component ID, it is not guaranteed to be one, and could be
    /// some other identifier of where to send invocations, representing one or more lattice entities.
    ///
    /// Lattice entities could be:
    /// - A (single) Component ID
    /// - A routing group
    /// - Some other opaque string
    #[allow(clippy::type_complexity)]
    pub instance_links: Arc<RwLock<HashMap<Box<str>, HashMap<Box<str>, Box<str>>>>>,
    /// Link name -> messaging client
    pub messaging_links: Arc<RwLock<HashMap<Box<str>, async_nats::Client>>>,

    pub invocation_timeout: Duration,
    /// Experimental features enabled in the host for gating handler functionality
    pub experimental_features: Features,
    /// Labels associated with the wasmCloud Host the component is running on
    pub host_labels: Arc<RwLock<BTreeMap<String, String>>>,
}

impl Handler {
    /// Used for creating a new handler from an existing one. This is different than clone because
    /// some fields shouldn't be copied between component instances such as link targets.
    pub fn copy_for_new(&self) -> Self {
        Handler {
            nats: self.nats.clone(),
            config_data: self.config_data.clone(),
            secrets: self.secrets.clone(),
            lattice: self.lattice.clone(),
            component_id: self.component_id.clone(),
            targets: Arc::default(),
            instance_links: self.instance_links.clone(),
            messaging_links: self.messaging_links.clone(),
            invocation_timeout: self.invocation_timeout,
            experimental_features: self.experimental_features,
            host_labels: self.host_labels.clone(),
        }
    }
}

#[async_trait]
impl Bus1_0_0 for Handler {
    /// Set the current link name in use by the handler, which is otherwise "default".
    ///
    /// Link names are important to set to differentiate similar operations (ex. `wasi:keyvalue/store.get`)
    /// that should go to different targets (ex. a capability provider like `kv-redis` vs `kv-vault`)
    #[instrument(level = "debug", skip(self))]
    async fn set_link_name(&self, link_name: String, interfaces: Vec<Arc<CallTargetInterface>>) {
        let interfaces = interfaces.iter().map(Deref::deref);
        let mut targets = self.targets.write().await;
        if link_name == "default" {
            for CallTargetInterface {
                namespace,
                package,
                interface,
            } in interfaces
            {
                targets.remove(&format!("{namespace}:{package}/{interface}").into_boxed_str());
            }
        } else {
            let link_name = Arc::from(link_name);
            for CallTargetInterface {
                namespace,
                package,
                interface,
            } in interfaces
            {
                targets.insert(
                    format!("{namespace}:{package}/{interface}").into_boxed_str(),
                    Arc::clone(&link_name),
                );
            }
        }
    }
}

#[async_trait]
impl Bus for Handler {
    /// Set the current link name in use by the handler, which is otherwise "default".
    ///
    /// Link names are important to set to differentiate similar operations (ex. `wasi:keyvalue/store.get`)
    /// that should go to different targets (ex. a capability provider like `kv-redis` vs `kv-vault`)
    #[instrument(level = "debug", skip(self))]
    async fn set_link_name(
        &self,
        link_name: String,
        interfaces: Vec<Arc<CallTargetInterface>>,
    ) -> anyhow::Result<Result<(), String>> {
        let links = self.instance_links.read().await;
        // Ensure that all interfaces have an established link with the given name.
        if let Some(interface_missing_link) = interfaces.iter().find_map(|i| {
            let instance = i.as_instance();
            // This could be expressed in one line as a `!(bool).then_some`, but the negation makes it confusing
            if links
                .get(link_name.as_str())
                .and_then(|l| l.get(instance.as_str()))
                .is_none()
            {
                Some(instance)
            } else {
                None
            }
        }) {
            return Ok(Err(format!(
                "interface `{interface_missing_link}` does not have an existing link with name `{link_name}`"
            )));
        }
        // Explicitly drop the lock before calling `set_link_name` just to avoid holding the lock for longer than needed
        drop(links);

        Bus1_0_0::set_link_name(self, link_name, interfaces).await;
        Ok(Ok(()))
    }
}

impl wrpc_transport::Invoke for Handler {
    type Context = Option<ReplacedInstanceTarget>;
    type Outgoing = <wrpc_transport_nats::Client as wrpc_transport::Invoke>::Outgoing;
    type Incoming = <wrpc_transport_nats::Client as wrpc_transport::Invoke>::Incoming;

    #[instrument(level = "debug", skip_all)]
    async fn invoke<P>(
        &self,
        target_instance: Self::Context,
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Self::Outgoing, Self::Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let links = self.instance_links.read().await;
        let targets = self.targets.read().await;

        let target_instance = match target_instance {
            Some(
                ReplacedInstanceTarget::BlobstoreBlobstore
                | ReplacedInstanceTarget::BlobstoreContainer,
            ) => "wasi:blobstore/blobstore",
            Some(ReplacedInstanceTarget::KeyvalueAtomics) => "wasi:keyvalue/atomics",
            Some(ReplacedInstanceTarget::KeyvalueStore) => "wasi:keyvalue/store",
            Some(ReplacedInstanceTarget::KeyvalueBatch) => "wasi:keyvalue/batch",
            Some(ReplacedInstanceTarget::KeyvalueWatch) => "wasi:keyvalue/watcher",
            Some(ReplacedInstanceTarget::HttpIncomingHandler) => "wasi:http/incoming-handler",
            Some(ReplacedInstanceTarget::HttpOutgoingHandler) => "wasi:http/outgoing-handler",
            None => instance.split_once('@').map_or(instance, |(l, _)| l),
        };

        let link_name = targets
            .get(target_instance)
            .map_or("default", AsRef::as_ref);

        let instances = links
            .get(link_name)
            .with_context(|| {
                warn!(
                    instance,
                    link_name,
                    ?target_instance,
                    ?self.component_id,
                    "no links with link name found for instance"
                );
                format!("link `{link_name}` not found for instance `{target_instance}`")
            })
            .map_err(Error::LinkNotFound)?;

        // Determine the lattice target ID we should be sending to
        let id = instances.get(target_instance).with_context(||{
            warn!(
                instance,
                ?target_instance,
                ?self.component_id,
                "component is not linked to a lattice target for the given instance"
            );
            format!("failed to call `{func}` in instance `{instance}` (failed to find a configured link with name `{link_name}` from component `{id}`, please check your configuration)", id = self.component_id)
        }).map_err(Error::LinkNotFound)?;

        let mut headers = injector_to_headers(&TraceContextInjector::default_with_span());
        headers.insert("source-id", &*self.component_id);
        headers.insert("link-name", link_name);
        let nats = wrpc_transport_nats::Client::new(
            Arc::clone(&self.nats),
            format!("{}.{id}", &self.lattice),
            None,
        )
        .await
        .map_err(Error::Handler)?;
        let (tx, rx) = nats
            .timeout(self.invocation_timeout)
            .invoke(Some(headers), instance, func, params, paths)
            .await
            .map_err(Error::Handler)?;
        Ok((tx, rx))
    }
}

#[async_trait]
impl Config for Handler {
    #[instrument(level = "debug", skip_all)]
    async fn get(
        &self,
        key: &str,
    ) -> anyhow::Result<Result<Option<String>, capability::config::store::Error>> {
        let lock = self.config_data.read().await;
        let conf = lock.get_config().await;
        let data = conf.get(key).cloned();
        Ok(Ok(data))
    }

    #[instrument(level = "debug", skip_all)]
    async fn get_all(
        &self,
    ) -> anyhow::Result<Result<Vec<(String, String)>, capability::config::store::Error>> {
        Ok(Ok(self
            .config_data
            .read()
            .await
            .get_config()
            .await
            .clone()
            .into_iter()
            .collect()))
    }
}

#[async_trait]
impl Logging for Handler {
    #[instrument(level = "trace", skip(self))]
    async fn log(
        &self,
        level: logging::Level,
        context: String,
        message: String,
    ) -> anyhow::Result<()> {
        match level {
            logging::Level::Trace => {
                tracing::event!(
                    tracing::Level::TRACE,
                    component_id = ?self.component_id,
                    level = level.to_string(),
                    context,
                    "{message}"
                );
            }
            logging::Level::Debug => {
                tracing::event!(
                    tracing::Level::DEBUG,
                    component_id = ?self.component_id,
                    level = level.to_string(),
                    context,
                    "{message}"
                );
            }
            logging::Level::Info => {
                tracing::event!(
                    tracing::Level::INFO,
                    component_id = ?self.component_id,
                    level = level.to_string(),
                    context,
                    "{message}"
                );
            }
            logging::Level::Warn => {
                tracing::event!(
                    tracing::Level::WARN,
                    component_id = ?self.component_id,
                    level = level.to_string(),
                    context,
                    "{message}"
                );
            }
            logging::Level::Error => {
                tracing::event!(
                    tracing::Level::ERROR,
                    component_id = ?self.component_id,
                    level = level.to_string(),
                    context,
                    "{message}"
                );
            }
            logging::Level::Critical => {
                tracing::event!(
                    tracing::Level::ERROR,
                    component_id = ?self.component_id,
                    level = level.to_string(),
                    context,
                    "{message}"
                );
            }
        };
        Ok(())
    }
}

#[async_trait]
impl Secrets for Handler {
    #[instrument(level = "debug", skip_all)]
    async fn get(
        &self,
        key: &str,
    ) -> anyhow::Result<Result<secrets::store::Secret, secrets::store::SecretsError>> {
        if self.secrets.read().await.get(key).is_some() {
            Ok(Ok(Arc::new(key.to_string())))
        } else {
            Ok(Err(secrets::store::SecretsError::NotFound))
        }
    }

    async fn reveal(
        &self,
        secret: secrets::store::Secret,
    ) -> anyhow::Result<secrets::store::SecretValue> {
        let read_lock = self.secrets.read().await;
        let Some(secret_val) = read_lock.get(secret.as_str()) else {
            // NOTE(brooksmtownsend): This error case should never happen, since we check for existence during `get` and
            // fail to start the component if the secret is missing. We might hit this during wRPC testing with resources.
            const ERROR_MSG: &str = "secret not found to reveal, ensure the secret is declared and associated with this component at startup";
            // NOTE: This "secret" is just the name of the key, not the actual secret value. Regardless the secret itself
            // both wasn't found and is wrapped by `secrecy` so it won't be logged.
            error!(?secret, ERROR_MSG);
            bail!(ERROR_MSG)
        };
        use secrecy::ExposeSecret;
        Ok(secret_val.expose_secret().clone())
    }
}

impl Messaging0_2 for Handler {
    #[instrument(level = "debug", skip_all)]
    async fn request(
        &self,
        subject: String,
        body: Vec<u8>,
        timeout_ms: u32,
    ) -> anyhow::Result<Result<messaging0_2_0::types::BrokerMessage, String>> {
        use wasmcloud_runtime::capability::wrpc::wasmcloud::messaging0_2_0 as messaging;

        {
            let targets = self.targets.read().await;
            let target = targets
                .get("wasmcloud:messaging/consumer")
                .map(AsRef::as_ref)
                .unwrap_or("default");
            if let Some(nats) = self.messaging_links.read().await.get(target) {
                match nats.request(subject, body.into()).await {
                    Ok(async_nats::Message {
                        subject,
                        payload,
                        reply,
                        ..
                    }) => {
                        return Ok(Ok(messaging0_2_0::types::BrokerMessage {
                            subject: subject.into_string(),
                            body: payload.into(),
                            reply_to: reply.map(async_nats::Subject::into_string),
                        }))
                    }
                    Err(err) => return Ok(Err(err.to_string())),
                }
            }
        }

        match messaging::consumer::request(self, None, &subject, &Bytes::from(body), timeout_ms)
            .await?
        {
            Ok(messaging::types::BrokerMessage {
                subject,
                body,
                reply_to,
            }) => Ok(Ok(messaging0_2_0::types::BrokerMessage {
                subject,
                body: body.into(),
                reply_to,
            })),
            Err(err) => Ok(Err(err)),
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn publish(
        &self,
        messaging0_2_0::types::BrokerMessage {
            subject,
            body,
            reply_to,
        }: messaging0_2_0::types::BrokerMessage,
    ) -> anyhow::Result<Result<(), String>> {
        use wasmcloud_runtime::capability::wrpc::wasmcloud::messaging0_2_0 as messaging;

        {
            let targets = self.targets.read().await;
            let target = targets
                .get("wasmcloud:messaging/consumer")
                .map(AsRef::as_ref)
                .unwrap_or("default");
            if let Some(nats) = self.messaging_links.read().await.get(target) {
                if let Some(reply_to) = reply_to {
                    match nats
                        .publish_with_reply(subject, reply_to, body.into())
                        .await
                    {
                        Ok(()) => return Ok(Ok(())),
                        Err(err) => return Ok(Err(err.to_string())),
                    }
                }
                match nats.publish(subject, body.into()).await {
                    Ok(()) => return Ok(Ok(())),
                    Err(err) => return Ok(Err(err.to_string())),
                }
            }
        }

        messaging::consumer::publish(
            self,
            None,
            &messaging::types::BrokerMessage {
                subject,
                body: body.into(),
                reply_to,
            },
        )
        .await
    }
}

struct MessagingClient {
    name: Box<str>,
}

#[async_trait]
impl MessagingClient0_3 for MessagingClient {
    async fn disconnect(&mut self) -> anyhow::Result<Result<(), messaging0_3_0::types::Error>> {
        Ok(Ok(()))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Concrete implementation of a message originating directly from the host, i.e. not received via
/// wRPC.
enum Message {
    Nats(async_nats::Message),
}

#[async_trait]
impl MessagingHostMessage0_3 for Message {
    async fn topic(&self) -> anyhow::Result<Option<messaging0_3_0::types::Topic>> {
        match self {
            Message::Nats(async_nats::Message { subject, .. }) => Ok(Some(subject.to_string())),
        }
    }
    async fn content_type(&self) -> anyhow::Result<Option<String>> {
        Ok(None)
    }
    async fn set_content_type(&mut self, _content_type: String) -> anyhow::Result<()> {
        bail!("`content-type` not supported")
    }
    async fn data(&self) -> anyhow::Result<Vec<u8>> {
        match self {
            Message::Nats(async_nats::Message { payload, .. }) => Ok(payload.to_vec()),
        }
    }
    async fn set_data(&mut self, buf: Vec<u8>) -> anyhow::Result<()> {
        match self {
            Message::Nats(msg) => {
                msg.payload = buf.into();
            }
        }
        Ok(())
    }
    async fn metadata(&self) -> anyhow::Result<Option<messaging0_3_0::types::Metadata>> {
        match self {
            Message::Nats(async_nats::Message { headers: None, .. }) => Ok(None),
            Message::Nats(async_nats::Message {
                headers: Some(headers),
                ..
            }) => Ok(Some(headers.iter().fold(
                // TODO: Initialize vector with capacity, once `async-nats` is updated to 0.37,
                // where `len` method is introduced:
                // https://docs.rs/async-nats/0.37.0/async_nats/header/struct.HeaderMap.html#method.len
                //Vec::with_capacity(headers.len()),
                Vec::default(),
                |mut headers, (k, vs)| {
                    for v in vs {
                        headers.push((k.to_string(), v.to_string()))
                    }
                    headers
                },
            ))),
        }
    }
    async fn add_metadata(&mut self, key: String, value: String) -> anyhow::Result<()> {
        match self {
            Message::Nats(async_nats::Message {
                headers: Some(headers),
                ..
            }) => {
                headers.append(key, value);
                Ok(())
            }
            Message::Nats(async_nats::Message { headers, .. }) => {
                *headers = Some(async_nats::HeaderMap::from_iter([(
                    key.into_header_name(),
                    value.into_header_value(),
                )]));
                Ok(())
            }
        }
    }
    async fn set_metadata(&mut self, meta: messaging0_3_0::types::Metadata) -> anyhow::Result<()> {
        match self {
            Message::Nats(async_nats::Message { headers, .. }) => {
                *headers = Some(
                    meta.into_iter()
                        .map(|(k, v)| (k.into_header_name(), v.into_header_value()))
                        .collect(),
                );
                Ok(())
            }
        }
    }
    async fn remove_metadata(&mut self, key: String) -> anyhow::Result<()> {
        match self {
            Message::Nats(async_nats::Message {
                headers: Some(headers),
                ..
            }) => {
                *headers = headers
                    .iter()
                    // NOTE(brooksmtownsend): The funky construction here is to provide a concrete type
                    // to the `as_ref()` call, which is necessary to satisfy the type inference on Windows.
                    .filter(|(k, ..)| (<&async_nats::HeaderName as AsRef<str>>::as_ref(k) != key))
                    .flat_map(|(k, vs)| zip(repeat(k.clone()), vs.iter().cloned()))
                    .collect();
                Ok(())
            }
            Message::Nats(..) => Ok(()),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl Messaging0_3 for Handler {
    #[instrument(level = "debug", skip_all)]
    async fn connect(
        &self,
        name: String,
    ) -> anyhow::Result<
        Result<Box<dyn MessagingClient0_3 + Send + Sync>, messaging0_3_0::types::Error>,
    > {
        Ok(Ok(Box::new(MessagingClient {
            name: name.into_boxed_str(),
        })))
    }

    #[instrument(level = "debug", skip_all)]
    async fn send(
        &self,
        client: &(dyn MessagingClient0_3 + Send + Sync),
        topic: messaging0_3_0::types::Topic,
        message: messaging0_3_0::types::Message,
    ) -> anyhow::Result<Result<(), messaging0_3_0::types::Error>> {
        use wasmcloud_runtime::capability::wrpc::wasmcloud::messaging0_2_0 as messaging;

        let MessagingClient { name } = client
            .as_any()
            .downcast_ref()
            .context("unknown client type")?;
        {
            let targets = self.targets.read().await;
            let target = targets
                .get("wasmcloud:messaging/producer")
                .map(AsRef::as_ref)
                .unwrap_or("default");
            let name = if name.is_empty() {
                "default"
            } else {
                name.as_ref()
            };
            if name != target {
                return Ok(Err(messaging0_3_0::types::Error::Other(format!(
                    "mismatch between link name and client connection name, `{name}` != `{target}`"
                ))));
            }
            if let Some(nats) = self.messaging_links.read().await.get(target) {
                match match message {
                    messaging0_3_0::types::Message::Host(message) => {
                        let message = message
                            .into_any()
                            .downcast::<Message>()
                            .map_err(|_| anyhow!("unknown message type"))?;
                        match *message {
                            Message::Nats(async_nats::Message {
                                payload,
                                headers: Some(headers),
                                ..
                            }) => nats.publish_with_headers(topic, headers, payload).await,
                            Message::Nats(async_nats::Message { payload, .. }) => {
                                nats.publish(topic, payload).await
                            }
                        }
                    }
                    messaging0_3_0::types::Message::Wrpc(messaging::types::BrokerMessage {
                        body,
                        ..
                    }) => nats.publish(topic, body).await,
                    messaging0_3_0::types::Message::Guest(MessagingGuestMessage0_3 {
                        content_type,
                        data,
                        metadata,
                    }) => {
                        if let Some(content_type) = content_type {
                            warn!(
                                content_type,
                                "`content-type` not supported by NATS.io, value is ignored"
                            );
                        }
                        if let Some(metadata) = metadata {
                            nats.publish_with_headers(
                                topic,
                                metadata
                                    .into_iter()
                                    .map(|(k, v)| (k.into_header_name(), v.into_header_value()))
                                    .collect(),
                                data.into(),
                            )
                            .await
                        } else {
                            nats.publish(topic, data.into()).await
                        }
                    }
                } {
                    Ok(()) => return Ok(Ok(())),
                    Err(err) => {
                        // TODO: Correctly handle error kind
                        return Ok(Err(messaging0_3_0::types::Error::Other(err.to_string())));
                    }
                }
            }
            let body = match message {
                messaging0_3_0::types::Message::Host(message) => {
                    let message = message
                        .into_any()
                        .downcast::<Message>()
                        .map_err(|_| anyhow!("unknown message type"))?;
                    match *message {
                        Message::Nats(async_nats::Message {
                            headers: Some(..), ..
                        }) => {
                            return Ok(Err(messaging0_3_0::types::Error::Other(
                                "headers not currently supported by wRPC targets".into(),
                            )));
                        }
                        Message::Nats(async_nats::Message { payload, .. }) => payload,
                    }
                }
                messaging0_3_0::types::Message::Wrpc(messaging::types::BrokerMessage {
                    body,
                    ..
                }) => body,
                messaging0_3_0::types::Message::Guest(MessagingGuestMessage0_3 {
                    metadata: Some(..),
                    ..
                }) => {
                    return Ok(Err(messaging0_3_0::types::Error::Other(
                        "`metadata` not currently supported by wRPC targets".into(),
                    )));
                }
                messaging0_3_0::types::Message::Guest(MessagingGuestMessage0_3 {
                    content_type,
                    data,
                    ..
                }) => {
                    if let Some(content_type) = content_type {
                        warn!(
                            content_type,
                            "`content-type` not currently supported by wRPC targets, value is ignored",
                        );
                    }
                    data.into()
                }
            };
            match messaging::consumer::publish(
                self,
                None,
                &messaging::types::BrokerMessage {
                    subject: topic,
                    body,
                    reply_to: None,
                },
            )
            .await
            {
                Ok(Ok(())) => Ok(Ok(())),
                Ok(Err(err)) => Ok(Err(messaging0_3_0::types::Error::Other(err))),
                // TODO: Correctly handle error kind
                Err(err) => Ok(Err(messaging0_3_0::types::Error::Other(err.to_string()))),
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn request(
        &self,
        client: &(dyn MessagingClient0_3 + Send + Sync),
        topic: messaging0_3_0::types::Topic,
        message: &messaging0_3_0::types::Message,
        options: Option<messaging0_3_0::request_reply::RequestOptions>,
    ) -> anyhow::Result<
        Result<Vec<Box<dyn MessagingHostMessage0_3 + Send + Sync>>, messaging0_3_0::types::Error>,
    > {
        if options.is_some() {
            return Ok(Err(messaging0_3_0::types::Error::Other(
                "`options` not currently supported".into(),
            )));
        }

        use wasmcloud_runtime::capability::wrpc::wasmcloud::messaging0_2_0 as messaging;

        let MessagingClient { name } = client
            .as_any()
            .downcast_ref()
            .context("unknown client type")?;
        {
            let targets = self.targets.read().await;
            let target = targets
                .get("wasmcloud:messaging/request-reply")
                .map(AsRef::as_ref)
                .unwrap_or("default");
            let name = if name.is_empty() {
                "default"
            } else {
                name.as_ref()
            };
            if name != target {
                return Ok(Err(messaging0_3_0::types::Error::Other(format!(
                    "mismatch between link name and client connection name, `{name}` != `{target}`"
                ))));
            }
            if let Some(nats) = self.messaging_links.read().await.get(target) {
                match match message {
                    messaging0_3_0::types::Message::Host(message) => {
                        let message = message
                            .as_any()
                            .downcast_ref::<Message>()
                            .context("unknown message type")?;
                        match message {
                            Message::Nats(async_nats::Message {
                                payload,
                                headers: Some(headers),
                                ..
                            }) => {
                                nats.request_with_headers(topic, headers.clone(), payload.clone())
                                    .await
                            }
                            Message::Nats(async_nats::Message { payload, .. }) => {
                                nats.request(topic, payload.clone()).await
                            }
                        }
                    }
                    messaging0_3_0::types::Message::Wrpc(messaging::types::BrokerMessage {
                        body,
                        ..
                    }) => nats.request(topic, body.clone()).await,
                    messaging0_3_0::types::Message::Guest(MessagingGuestMessage0_3 {
                        content_type,
                        data,
                        metadata,
                    }) => {
                        if let Some(content_type) = content_type {
                            warn!(
                                content_type,
                                "`content-type` not supported by NATS.io, value is ignored"
                            );
                        }
                        if let Some(metadata) = metadata {
                            nats.request_with_headers(
                                topic,
                                metadata
                                    .iter()
                                    .map(|(k, v)| {
                                        (
                                            k.as_str().into_header_name(),
                                            v.as_str().into_header_value(),
                                        )
                                    })
                                    .collect(),
                                Bytes::copy_from_slice(data),
                            )
                            .await
                        } else {
                            nats.request(topic, Bytes::copy_from_slice(data)).await
                        }
                    }
                } {
                    Ok(msg) => return Ok(Ok(vec![Box::new(Message::Nats(msg))])),
                    Err(err) => {
                        // TODO: Correctly handle error kind
                        return Ok(Err(messaging0_3_0::types::Error::Other(err.to_string())));
                    }
                }
            }
            let body = match message {
                messaging0_3_0::types::Message::Host(message) => {
                    let message = message
                        .as_any()
                        .downcast_ref::<Message>()
                        .context("unknown message type")?;
                    match message {
                        Message::Nats(async_nats::Message {
                            headers: Some(..), ..
                        }) => {
                            return Ok(Err(messaging0_3_0::types::Error::Other(
                                "headers not currently supported by wRPC targets".into(),
                            )));
                        }
                        Message::Nats(async_nats::Message { payload, .. }) => payload.clone(),
                    }
                }
                messaging0_3_0::types::Message::Wrpc(messaging::types::BrokerMessage {
                    body,
                    ..
                }) => body.clone(),
                messaging0_3_0::types::Message::Guest(MessagingGuestMessage0_3 {
                    metadata: Some(..),
                    ..
                }) => {
                    return Ok(Err(messaging0_3_0::types::Error::Other(
                        "`metadata` not currently supported by wRPC targets".into(),
                    )));
                }
                messaging0_3_0::types::Message::Guest(MessagingGuestMessage0_3 {
                    content_type,
                    data,
                    ..
                }) => {
                    if let Some(content_type) = content_type {
                        warn!(
                            content_type,
                            "`content-type` not currently supported by wRPC targets, value is ignored",
                        );
                    }
                    Bytes::copy_from_slice(data)
                }
            };

            match messaging::consumer::publish(
                self,
                None,
                &messaging::types::BrokerMessage {
                    subject: topic,
                    body,
                    reply_to: None,
                },
            )
            .await
            {
                Ok(Ok(())) => Ok(Err(messaging0_3_0::types::Error::Other(
                    "message sent, but returning responses is not currently supported by wRPC targets".into(),
                ))),
                Ok(Err(err)) => Ok(Err(messaging0_3_0::types::Error::Other(err))),
                // TODO: Correctly handle error kind
                Err(err) => Ok(Err(messaging0_3_0::types::Error::Other(err.to_string()))),
            }
        }
    }

    #[instrument(level = "debug", skip_all)]
    async fn reply(
        &self,
        reply_to: &messaging0_3_0::types::Message,
        message: messaging0_3_0::types::Message,
    ) -> anyhow::Result<Result<(), messaging0_3_0::types::Error>> {
        use wasmcloud_runtime::capability::wrpc::wasmcloud::messaging0_2_0 as messaging;

        {
            let targets = self.targets.read().await;
            let target = targets
                .get("wasmcloud:messaging/request-reply")
                .map(AsRef::as_ref)
                .unwrap_or("default");
            if let Some(nats) = self.messaging_links.read().await.get(target) {
                let subject = match reply_to {
                    messaging0_3_0::types::Message::Host(reply_to) => {
                        match reply_to
                            .as_any()
                            .downcast_ref::<Message>()
                            .context("unknown message type")?
                        {
                            Message::Nats(async_nats::Message {
                                reply: Some(reply), ..
                            }) => reply.clone(),
                            Message::Nats(async_nats::Message { reply: None, .. }) => {
                                return Ok(Err(messaging0_3_0::types::Error::Other(
                                    "reply not set in incoming NATS.io message".into(),
                                )))
                            }
                        }
                    }
                    messaging0_3_0::types::Message::Wrpc(messaging::types::BrokerMessage {
                        reply_to: Some(reply_to),
                        ..
                    }) => reply_to.as_str().into(),
                    messaging0_3_0::types::Message::Wrpc(messaging::types::BrokerMessage {
                        reply_to: None,
                        ..
                    }) => {
                        return Ok(Err(messaging0_3_0::types::Error::Other(
                            "reply not set in incoming wRPC message".into(),
                        )))
                    }
                    messaging0_3_0::types::Message::Guest(..) => {
                        return Ok(Err(messaging0_3_0::types::Error::Other(
                            "cannot reply to guest message".into(),
                        )))
                    }
                };
                match match message {
                    messaging0_3_0::types::Message::Host(message) => {
                        let message = message
                            .into_any()
                            .downcast::<Message>()
                            .map_err(|_| anyhow!("unknown message type"))?;
                        match *message {
                            Message::Nats(async_nats::Message {
                                payload,
                                headers: Some(headers),
                                ..
                            }) => nats.publish_with_headers(subject, headers, payload).await,
                            Message::Nats(async_nats::Message { payload, .. }) => {
                                nats.publish(subject, payload).await
                            }
                        }
                    }
                    messaging0_3_0::types::Message::Wrpc(messaging::types::BrokerMessage {
                        body,
                        ..
                    }) => nats.publish(subject, body).await,
                    messaging0_3_0::types::Message::Guest(MessagingGuestMessage0_3 {
                        content_type,
                        data,
                        metadata,
                    }) => {
                        if let Some(content_type) = content_type {
                            warn!(
                                content_type,
                                "`content-type` not supported by NATS.io, value is ignored"
                            );
                        }
                        if let Some(metadata) = metadata {
                            nats.publish_with_headers(
                                subject,
                                metadata
                                    .into_iter()
                                    .map(|(k, v)| (k.into_header_name(), v.into_header_value()))
                                    .collect(),
                                data.into(),
                            )
                            .await
                        } else {
                            nats.publish(subject, data.into()).await
                        }
                    }
                } {
                    Ok(()) => return Ok(Ok(())),
                    Err(err) => {
                        // TODO: Correctly handle error kind
                        return Ok(Err(messaging0_3_0::types::Error::Other(err.to_string())));
                    }
                }
            }
            let body = match message {
                messaging0_3_0::types::Message::Host(message) => {
                    let message = message
                        .into_any()
                        .downcast::<Message>()
                        .map_err(|_| anyhow!("unknown message type"))?;
                    match *message {
                        Message::Nats(async_nats::Message {
                            headers: Some(..), ..
                        }) => {
                            return Ok(Err(messaging0_3_0::types::Error::Other(
                                "headers not currently supported by wRPC targets".into(),
                            )));
                        }
                        Message::Nats(async_nats::Message { payload, .. }) => payload,
                    }
                }
                messaging0_3_0::types::Message::Wrpc(messaging::types::BrokerMessage {
                    body,
                    ..
                }) => body,
                messaging0_3_0::types::Message::Guest(MessagingGuestMessage0_3 {
                    metadata: Some(..),
                    ..
                }) => {
                    return Ok(Err(messaging0_3_0::types::Error::Other(
                        "`metadata` not currently supported by wRPC targets".into(),
                    )));
                }
                messaging0_3_0::types::Message::Guest(MessagingGuestMessage0_3 {
                    content_type,
                    data,
                    ..
                }) => {
                    if let Some(content_type) = content_type {
                        warn!(
                            content_type,
                            "`content-type` not currently supported by wRPC targets, value is ignored",
                        );
                    }
                    data.into()
                }
            };
            let subject = match reply_to {
                messaging0_3_0::types::Message::Host(reply_to) => {
                    match reply_to
                        .as_any()
                        .downcast_ref::<Message>()
                        .context("unknown message type")?
                    {
                        Message::Nats(async_nats::Message {
                            reply: Some(reply), ..
                        }) => reply.to_string(),
                        Message::Nats(async_nats::Message { reply: None, .. }) => {
                            return Ok(Err(messaging0_3_0::types::Error::Other(
                                "reply not set in incoming NATS.io message".into(),
                            )))
                        }
                    }
                }
                messaging0_3_0::types::Message::Wrpc(messaging::types::BrokerMessage {
                    reply_to: Some(reply_to),
                    ..
                }) => reply_to.clone(),
                messaging0_3_0::types::Message::Wrpc(messaging::types::BrokerMessage {
                    reply_to: None,
                    ..
                }) => {
                    return Ok(Err(messaging0_3_0::types::Error::Other(
                        "reply not set in incoming wRPC message".into(),
                    )))
                }
                messaging0_3_0::types::Message::Guest(..) => {
                    return Ok(Err(messaging0_3_0::types::Error::Other(
                        "cannot reply to guest message".into(),
                    )))
                }
            };
            match messaging::consumer::publish(
                self,
                None,
                &messaging::types::BrokerMessage {
                    subject,
                    body,
                    reply_to: None,
                },
            )
            .await
            {
                Ok(Ok(())) => Ok(Ok(())),
                Ok(Err(err)) => Ok(Err(messaging0_3_0::types::Error::Other(err))),
                // TODO: Correctly handle error kind
                Err(err) => Ok(Err(messaging0_3_0::types::Error::Other(err.to_string()))),
            }
        }
    }
}

#[async_trait]
impl Identity for Handler {
    #[cfg(unix)]
    #[instrument(level = "debug", skip_all)]
    async fn get(
        &self,
        audience: &str,
    ) -> anyhow::Result<Result<Option<String>, identity::store::Error>> {
        let mut client = match DelegatedIdentityClient::default().await {
            Ok(client) => client,
            Err(err) => {
                return Ok(Err(identity::store::Error::Io(format!(
                    "Unable to connect to workload identity service: {err}"
                ))));
            }
        };

        let mut selectors =
            parse_selectors_from_host_labels(self.host_labels.read().await.deref()).await;
        // "wasmcloud", "component:{component_id}" is inserted at the end to make sure it can't be overridden.
        selectors.push(Selector::Generic((
            WASMCLOUD_SELECTOR_TYPE.to_string(),
            format!("{}:{}", WASMCLOUD_SELECTOR_COMPONENT, self.component_id),
        )));

        let svids = match client
            .fetch_jwt_svids(&[audience], Selectors(selectors))
            .await
        {
            Ok(svids) => svids,
            Err(err) => {
                return Ok(Err(identity::store::Error::Io(format!(
                    "Unable to query workload identity service: {err}"
                ))));
            }
        };

        if !svids.is_empty() {
            // TODO: Is there a better way to determine which SVID to return here?
            let svid = svids.first().map(|svid| svid.token()).unwrap_or_default();
            Ok(Ok(Some(svid.to_string())))
        } else {
            Ok(Err(identity::store::Error::NotFound))
        }
    }

    #[cfg(target_family = "windows")]
    #[instrument(level = "debug", skip_all)]
    async fn get(
        &self,
        _audience: &str,
    ) -> anyhow::Result<Result<Option<String>, identity::store::Error>> {
        Ok(Err(identity::store::Error::Other(
            "workload identity is not supported on Windows".to_string(),
        )))
    }
}

impl InvocationErrorIntrospect for Handler {
    fn invocation_error_kind(&self, err: &anyhow::Error) -> InvocationErrorKind {
        if let Some(err) = err.root_cause().downcast_ref::<std::io::Error>() {
            if err.kind() == std::io::ErrorKind::NotConnected {
                return InvocationErrorKind::NotFound;
            }
        }
        InvocationErrorKind::Trap
    }
}

// TODO(joonas): Make this more generalized so we can support non-wasmcloud-specific
// selectors as well.
//
// environment variable -> WASMCLOUD_LABEL_wasmcloud__ns=my-namespace-goes-here
// becomes:
// SPIRE Selector -> wasmcloud:ns:my-namespace-goes-here
#[cfg(unix)]
async fn parse_selectors_from_host_labels(host_labels: &BTreeMap<String, String>) -> Vec<Selector> {
    let mut selectors = vec![];

    for (key, value) in host_labels.iter() {
        // Ensure the label starts with `wasmcloud__` and doesn't end in `__`, i.e. just `wasmcloud__`
        if key.starts_with("wasmcloud__") && !key.ends_with("__") {
            let selector = key
                // Replace all __ with :
                .replace("__", ":")
                // Remove the leading "wasmcloud"
                .split_once(":")
                // Map the remaining part of the label key together with the value `` to make it a selector
                .map(|(_, selector)| format!("{}:{}", selector, value))
                // This should never get triggered, but just in case.
                .unwrap_or("unknown".to_string());

            selectors.push(Selector::Generic((
                WASMCLOUD_SELECTOR_TYPE.to_string(),
                selector,
            )));
        }
    }

    selectors
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::env::consts::{ARCH, FAMILY, OS};

    #[tokio::test]
    async fn test_parse_selectors_from_host_labels() {
        let labels = BTreeMap::from([
            ("hostcore.arch".into(), ARCH.into()),
            ("hostcore.os".into(), OS.into()),
            ("hostcore.osfamily".into(), FAMILY.into()),
            ("wasmcloud__lattice".into(), "default".into()),
        ]);

        let selectors = parse_selectors_from_host_labels(&labels).await;

        assert_eq!(selectors.len(), 1);

        let (selector_type, selector_value) = match selectors.first() {
            Some(Selector::Generic(pair)) => pair,
            _ => &("wrong-value".into(), "wrong-value".into()),
        };
        assert_eq!(selector_type, WASMCLOUD_SELECTOR_TYPE);
        assert_eq!(selector_value, "lattice:default");
    }

    #[tokio::test]
    async fn test_parse_selectors_from_host_labels_defaults_to_no_selectors() {
        let no_labels = BTreeMap::new();
        let selectors = parse_selectors_from_host_labels(&no_labels).await;
        assert_eq!(selectors.len(), 0);
    }
}
