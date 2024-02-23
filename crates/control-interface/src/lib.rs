//! # Control Interface Client
//!
//! This library provides a client API for consuming the wasmCloud control interface over a
//! NATS connection. This library can be used by multiple types of tools, and is also used
//! by the control interface capability provider and the wash CLI

mod broker;
mod otel;
mod types;

use async_nats::Subscriber;
pub use types::*;

use core::fmt::{self, Debug};
use core::time::Duration;

use std::collections::HashMap;

use cloudevents::event::Event;
use futures::{StreamExt, TryFutureExt};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Receiver;
use tracing::{debug, error, instrument, trace};

type Result<T> = ::std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Lattice control interface client
#[derive(Clone)]
pub struct Client {
    nc: async_nats::Client,
    topic_prefix: Option<String>,
    /// Lattice prefix
    pub lattice: String,
    timeout: Duration,
    auction_timeout: Duration,
}

impl Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("topic_prefix", &self.topic_prefix)
            .field("lattice", &self.lattice)
            .field("timeout", &self.timeout)
            .field("auction_timeout", &self.auction_timeout)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Get a copy of the NATS client in use by this control client
    #[allow(unused)]
    pub fn nats_client(&self) -> async_nats::Client {
        self.nc.clone()
    }
}

/// A client builder that can be used to fluently provide configuration settings used to construct
/// the control interface client
pub struct ClientBuilder {
    nc: async_nats::Client,
    topic_prefix: Option<String>,
    lattice: String,
    timeout: Duration,
    auction_timeout: Duration,
}

impl ClientBuilder {
    /// Creates a new client builder using the given client with all configuration values set to
    /// their defaults
    #[must_use]
    pub fn new(nc: async_nats::Client) -> ClientBuilder {
        ClientBuilder {
            nc,
            topic_prefix: None,
            lattice: "default".to_string(),
            timeout: Duration::from_secs(2),
            auction_timeout: Duration::from_secs(5),
        }
    }

    /// Sets the topic prefix for the NATS topic used for all control requests. Not to be confused
    /// with lattice ID/prefix
    #[must_use]
    pub fn topic_prefix(self, prefix: impl Into<String>) -> ClientBuilder {
        ClientBuilder {
            topic_prefix: Some(prefix.into()),
            ..self
        }
    }

    /// The lattice ID/prefix used for this client. If this function is not invoked, the prefix will
    /// be set to `default`
    #[must_use]
    pub fn lattice(self, prefix: impl Into<String>) -> ClientBuilder {
        ClientBuilder {
            lattice: prefix.into(),
            ..self
        }
    }

    /// Sets the timeout for control interface requests issued by the client. If not set, the
    /// default will be 2 seconds
    #[must_use]
    pub fn timeout(self, timeout: Duration) -> ClientBuilder {
        ClientBuilder { timeout, ..self }
    }

    /// Sets the timeout for auction (scatter/gather) operations. If not set, the default will be 5
    /// seconds
    #[must_use]
    pub fn auction_timeout(self, timeout: Duration) -> ClientBuilder {
        ClientBuilder {
            auction_timeout: timeout,
            ..self
        }
    }

    /// Constructs the client with the given configuration from the builder
    #[must_use]
    pub fn build(self) -> Client {
        Client {
            nc: self.nc,
            topic_prefix: self.topic_prefix,
            lattice: self.lattice,
            timeout: self.timeout,
            auction_timeout: self.auction_timeout,
        }
    }
}

impl Client {
    /// Convenience method for creating a new client with all default settings. This is the same as
    /// calling `ClientBuilder::new(nc).build()`
    #[must_use]
    pub fn new(nc: async_nats::Client) -> Client {
        ClientBuilder::new(nc).build()
    }

    #[instrument(level = "debug", skip_all)]
    pub(crate) async fn request_timeout(
        &self,
        subject: String,
        payload: Vec<u8>,
        timeout: Duration,
    ) -> Result<async_nats::Message> {
        match tokio::time::timeout(
            timeout,
            self.nc.request_with_headers(
                subject,
                otel::HeaderInjector::default_with_span().into(),
                payload.into(),
            ),
        )
        .await
        {
            Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out").into()),
            Ok(Ok(message)) => Ok(message),
            Ok(Err(e)) => Err(e.into()),
        }
    }

    /// Queries the lattice for all responsive hosts, waiting for the full period specified by
    /// _timeout_.
    #[instrument(level = "debug", skip_all)]
    pub async fn get_hosts(&self) -> Result<Vec<Host>> {
        let subject = broker::queries::hosts(&self.topic_prefix, &self.lattice);
        debug!("get_hosts:publish {}", &subject);
        self.publish_and_wait(subject, Vec::new()).await
    }

    /// Retrieves the contents of a running host
    #[instrument(level = "debug", skip_all)]
    pub async fn get_host_inventory(&self, host_id: &str) -> Result<HostInventory> {
        let subject = broker::queries::host_inventory(
            &self.topic_prefix,
            &self.lattice,
            parse_identifier(&IdentifierKind::HostId, host_id)?.as_str(),
        );
        debug!("get_host_inventory:request {}", &subject);
        match self.request_timeout(subject, vec![], self.timeout).await {
            Ok(msg) => Ok(json_deserialize(&msg.payload)?),
            Err(e) => Err(format!("Did not receive host inventory from target host: {e}").into()),
        }
    }

    /// Retrieves the full set of all cached claims in the lattice.   
    #[instrument(level = "debug", skip_all)]
    pub async fn get_claims(&self) -> Result<Vec<HashMap<String, String>>> {
        let subject = broker::queries::claims(&self.topic_prefix, &self.lattice);
        debug!("get_claims:request {}", &subject);
        match self.request_timeout(subject, vec![], self.timeout).await {
            Ok(msg) => {
                let list: GetClaimsResponse = json_deserialize(&msg.payload)?;
                Ok(list.claims)
            }
            Err(e) => Err(format!("Did not receive claims from lattice: {e}").into()),
        }
    }

    /// Performs an actor auction within the lattice, publishing a set of constraints and the
    /// metadata for the actor in question. This will always wait for the full period specified by
    /// _duration_, and then return the set of gathered results. It is then up to the client to
    /// choose from among the "auction winners" to issue the appropriate command to start an actor.
    /// Clients cannot assume that auctions will always return at least one result.
    #[instrument(level = "debug", skip_all)]
    pub async fn perform_actor_auction(
        &self,
        actor_ref: &str,
        actor_id: &str,
        constraints: HashMap<String, String>,
    ) -> Result<Vec<ActorAuctionAck>> {
        let subject = broker::actor_auction_subject(&self.topic_prefix, &self.lattice);
        let bytes = json_serialize(ActorAuctionRequest {
            actor_ref: parse_identifier(&IdentifierKind::ActorRef, actor_ref)?,
            actor_id: parse_identifier(&IdentifierKind::ComponentId, actor_id)?,
            constraints,
        })?;
        debug!("actor_auction:publish {}", &subject);
        self.publish_and_wait(subject, bytes).await
    }

    /// Performs a provider auction within the lattice, publishing a set of constraints and the
    /// metadata for the provider in question. This will always wait for the full period specified
    /// by _duration_, and then return the set of gathered results. It is then up to the client to
    /// choose from among the "auction winners" and issue the appropriate command to start a
    /// provider. Clients cannot assume that auctions will always return at least one result.
    #[instrument(level = "debug", skip_all)]
    pub async fn perform_provider_auction(
        &self,
        provider_ref: &str,
        provider_id: &str,
        constraints: HashMap<String, String>,
    ) -> Result<Vec<ProviderAuctionAck>> {
        let subject = broker::provider_auction_subject(&self.topic_prefix, &self.lattice);
        let bytes = json_serialize(ProviderAuctionRequest {
            provider_ref: parse_identifier(&IdentifierKind::ProviderRef, provider_ref)?,
            provider_id: parse_identifier(&IdentifierKind::ComponentId, provider_id)?,
            constraints,
        })?;
        debug!("provider_auction:publish {}", &subject);
        self.publish_and_wait(subject, bytes).await
    }

    /// Sends a request to the given host to scale a given actor. This returns an acknowledgement of
    /// _receipt_ of the command, not a confirmation that the actor scaled. An acknowledgement will
    /// either indicate some form of validation failure, or, if no failure occurs, the receipt of
    /// the command. To avoid blocking consumers, wasmCloud hosts will acknowledge the scale actor
    /// command prior to fetching the actor's OCI bytes. If a client needs deterministic results as
    /// to whether the actor completed its startup process, the client will have to monitor the
    /// appropriate event in the control event stream
    ///
    /// # Arguments
    /// `host_id`: The ID of the host to scale the actor on
    /// `actor_ref`: The OCI reference of the actor to scale
    /// `max_instances`: The maximum number of instances this actor can run concurrently. Specifying `0` will stop the actor.
    /// `annotations`: Optional annotations to apply to the actor
    #[instrument(level = "debug", skip_all)]
    pub async fn scale_actor(
        &self,
        host_id: &str,
        actor_ref: &str,
        actor_id: &str,
        max_instances: u32,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let host_id = parse_identifier(&IdentifierKind::HostId, host_id)?;
        let subject =
            broker::commands::scale_actor(&self.topic_prefix, &self.lattice, host_id.as_str());
        debug!("scale_actor:request {}", &subject);
        let bytes = json_serialize(ScaleActorCommand {
            max_instances,
            actor_ref: parse_identifier(&IdentifierKind::ActorRef, actor_ref)?,
            actor_id: parse_identifier(&IdentifierKind::ComponentId, actor_id)?,
            host_id,
            annotations,
        })?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => Ok(json_deserialize(&msg.payload)?),
            Err(e) => Err(format!("Did not receive scale actor acknowledgement: {e}").into()),
        }
    }

    /// Publishes a registry credential map to the control interface of the lattice. All hosts will
    /// be listening and all will overwrite their registry credential map with the new information.
    /// It is highly recommended you use TLS connections with NATS and isolate the control interface
    /// credentials when using this function in production as the data contains secrets
    #[instrument(level = "debug", skip_all)]
    pub async fn put_registries(&self, registries: RegistryCredentialMap) -> Result<()> {
        let subject = broker::publish_registries(&self.topic_prefix, &self.lattice);
        debug!("put_registries:publish {}", &subject);
        let bytes = json_serialize(&registries)?;
        let resp = self
            .nc
            .publish_with_headers(
                subject,
                otel::HeaderInjector::default_with_span().into(),
                bytes.into(),
            )
            .await;
        if let Err(e) = resp {
            Err(format!("Failed to push registry credential map: {e}").into())
        } else {
            Ok(())
        }
    }

    /// Puts a link into the lattice. Returns an error if it was unable to put the link
    #[instrument(level = "debug", skip_all)]
    pub async fn advertise_link(
        &self,
        source_id: &str,
        target: &str,
        link_name: &str,
        wit_namespace: &str,
        wit_package: &str,
        interfaces: Vec<String>,
        source_config: Vec<String>,
        target_config: Vec<String>,
    ) -> Result<CtlOperationAck> {
        let ld = InterfaceLinkDefinition {
            source_id: parse_identifier(&IdentifierKind::ComponentId, source_id)?,
            target: parse_identifier(&IdentifierKind::ComponentId, target)?,
            name: parse_identifier(&IdentifierKind::LinkName, link_name)?,
            wit_namespace: wit_namespace.to_string(),
            wit_package: wit_package.to_string(),
            interfaces,
            source_config,
            target_config,
        };

        let subject = broker::advertise_link(&self.topic_prefix, &self.lattice);
        debug!("advertise_link:request {}", &subject);

        let bytes = crate::json_serialize(&ld)?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => Ok(json_deserialize(&msg.payload)?),
            Err(e) => Err(format!("Did not receive advertise link acknowledgement: {e}").into()),
        }
    }

    /// Removes a link from the lattice metadata keyvalue bucket. Returns an error if it was unable
    /// to delete. This is an idempotent operation.
    #[instrument(level = "debug", skip_all)]
    pub async fn remove_link(
        &self,
        source_id: &str,
        link_name: &str,
        wit_namespace: &str,
        wit_package: &str,
    ) -> Result<CtlOperationAck> {
        let subject = broker::remove_link(&self.topic_prefix, &self.lattice);
        let ld = RemoveInterfaceLinkDefinitionRequest {
            source_id: parse_identifier(&IdentifierKind::ComponentId, source_id)?,
            name: parse_identifier(&IdentifierKind::LinkName, link_name)?,
            wit_namespace: wit_namespace.to_string(),
            wit_package: wit_package.to_string(),
        };
        let bytes = crate::json_serialize(&ld)?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => Ok(json_deserialize(&msg.payload)?),
            Err(e) => Err(format!("Did not receive remove link acknowledgement: {e}").into()),
        }
    }

    /// Retrieves the list of link definitions stored in the lattice metadata key-value bucket. If
    /// the client was created with caching, this will return the cached list of links. Otherwise,
    /// it will query the bucket for the list of links.
    #[instrument(level = "debug", skip_all)]
    pub async fn query_links(&self) -> Result<Vec<InterfaceLinkDefinition>> {
        let subject = broker::queries::link_definitions(&self.topic_prefix, &self.lattice);
        debug!("query_links:request {}", &subject);
        match self.request_timeout(subject, vec![], self.timeout).await {
            Ok(msg) => {
                let links: Vec<InterfaceLinkDefinition> = json_deserialize(&msg.payload)?;
                Ok(links)
            }
            Err(e) => Err(format!("Did not receive a response to links query: {e}").into()),
        }
    }

    /// Puts a named config, replacing any data that is already present.
    ///
    /// Config names must be valid NATS subject strings and not contain any `.` or `>` characters.
    #[instrument(level = "debug", skip_all)]
    pub async fn put_config(
        &self,
        config_name: &str,
        config: impl Into<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::put_config(&self.topic_prefix, &self.lattice, config_name);
        debug!(%subject, %config_name, "Putting config");
        let data = serde_json::to_vec(&config.into())?;
        match self.request_timeout(subject, data, self.timeout).await {
            Ok(msg) => json_deserialize(&msg.payload),
            Err(e) => Err(format!("Did not receive a response to put config request: {e}").into()),
        }
    }

    /// Delete the named config item.
    ///
    /// Config names must be valid NATS subject strings and not contain any `.` or `>` characters.
    #[instrument(level = "debug", skip_all)]
    pub async fn delete_config(&self, config_name: &str) -> Result<CtlOperationAck> {
        let subject = broker::delete_config(&self.topic_prefix, &self.lattice, config_name);
        debug!(%subject, %config_name, "Delete config");
        match self
            .request_timeout(subject, Vec::default(), self.timeout)
            .await
        {
            Ok(msg) => json_deserialize(&msg.payload),
            Err(e) => {
                Err(format!("Did not receive a response to delete config request: {e}").into())
            }
        }
    }

    /// Get the named config item.
    ///
    /// Config names must be valid NATS subject strings and not contain any `.` or `>` characters.
    #[instrument(level = "debug", skip_all)]
    pub async fn get_config(&self, config_name: &str) -> Result<GetConfigResponse> {
        let subject = broker::queries::config(&self.topic_prefix, &self.lattice, config_name);
        debug!(%subject, %config_name, "Getting config");
        match self
            .request_timeout(subject, Vec::default(), self.timeout)
            .await
        {
            Ok(msg) => json_deserialize(&msg.payload),
            Err(e) => Err(format!("Did not receive a response to get config request: {e}").into()),
        }
    }

    /// Put a new (or update an existing) label on the given host.
    ///
    /// # Errors
    ///
    /// Will return an error if there is a communication problem with the host
    pub async fn put_label(
        &self,
        host_id: &str,
        key: &str,
        value: &str,
    ) -> Result<CtlOperationAck> {
        let subject = broker::put_label(&self.topic_prefix, &self.lattice, host_id);
        debug!(%subject, "putting label");
        let bytes = json_serialize(HostLabel {
            key: key.to_string(),
            value: value.to_string(),
        })?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => Ok(json_deserialize(&msg.payload)?),
            Err(e) => Err(format!("Did not receive put label acknowledgement: {e}").into()),
        }
    }

    /// Removes a label from the given host.
    ///
    /// # Errors
    ///
    /// Will return an error if there is a communication problem with the host
    pub async fn delete_label(&self, host_id: &str, key: &str) -> Result<CtlOperationAck> {
        let subject = broker::delete_label(&self.topic_prefix, &self.lattice, host_id);
        debug!(%subject, "removing label");
        let bytes = json_serialize(HostLabel {
            key: key.to_string(),
            value: String::new(), // value isn't parsed by the host
        })?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => Ok(json_deserialize(&msg.payload)?),
            Err(e) => Err(format!("Did not receive remove label acknowledgement: {e}").into()),
        }
    }

    /// Issue a command to a host instructing that it replace an existing actor (indicated by its
    /// public key) with a new actor indicated by an OCI image reference. The host will acknowledge
    /// this request as soon as it verifies that the target actor is running. This acknowledgement
    /// occurs **before** the new bytes are downloaded. Live-updating an actor can take a long time
    /// and control clients cannot block waiting for a reply that could come several seconds later.
    /// If you need to verify that the actor has been updated, you will want to set up a listener
    /// for the appropriate **PublishedEvent** which will be published on the control events channel
    /// in JSON
    #[instrument(level = "debug", skip_all)]
    pub async fn update_actor(
        &self,
        host_id: &str,
        existing_actor_id: &str,
        new_actor_ref: &str,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let host_id = parse_identifier(&IdentifierKind::HostId, host_id)?;
        let subject =
            broker::commands::update_actor(&self.topic_prefix, &self.lattice, host_id.as_str());
        debug!("update_actor:request {}", &subject);
        let bytes = json_serialize(UpdateActorCommand {
            host_id,
            actor_id: parse_identifier(&IdentifierKind::ComponentId, existing_actor_id)?,
            new_actor_ref: parse_identifier(&IdentifierKind::ActorRef, new_actor_ref)?,
            annotations,
        })?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => Ok(json_deserialize(&msg.payload)?),
            Err(e) => Err(format!("Did not receive update actor acknowledgement: {e}").into()),
        }
    }

    /// Issues a command to a host to start a provider with a given OCI reference using the
    /// specified link name (or "default" if none is specified). The target wasmCloud host will
    /// acknowledge the receipt of this command _before_ downloading the provider's bytes from the
    /// OCI registry, indicating either a validation failure or success. If a client needs
    /// deterministic guarantees that the provider has completed its startup process, such a client
    /// needs to monitor the control event stream for the appropriate event.
    #[instrument(level = "debug", skip_all)]
    pub async fn start_provider(
        &self,
        host_id: &str,
        provider_ref: &str,
        provider_id: &str,
        annotations: Option<HashMap<String, String>>,
        provider_configuration: Option<String>,
    ) -> Result<CtlOperationAck> {
        let host_id = parse_identifier(&IdentifierKind::HostId, host_id)?;
        let subject =
            broker::commands::start_provider(&self.topic_prefix, &self.lattice, host_id.as_str());
        debug!("start_provider:request {}", &subject);
        let bytes = json_serialize(StartProviderCommand {
            host_id,
            provider_ref: parse_identifier(&IdentifierKind::ProviderRef, provider_ref)?,
            provider_id: parse_identifier(&IdentifierKind::ComponentId, provider_id)?,
            annotations,
            configuration: provider_configuration,
        })?;

        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => Ok(json_deserialize(&msg.payload)?),
            Err(e) => Err(format!("Did not receive start provider acknowledgement: {e}").into()),
        }
    }

    /// Issues a command to a host to stop a provider for the given OCI reference, link name, and
    /// contract ID. The target wasmCloud host will acknowledge the receipt of this command, and
    /// _will not_ supply a discrete confirmation that a provider has terminated. For that kind of
    /// information, the client must also monitor the control event stream
    #[instrument(level = "debug", skip_all)]
    pub async fn stop_provider(
        &self,
        host_id: &str,
        provider_id: &str,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let host_id = parse_identifier(&IdentifierKind::HostId, host_id)?;

        let subject =
            broker::commands::stop_provider(&self.topic_prefix, &self.lattice, host_id.as_str());
        debug!("stop_provider:request {}", &subject);
        let bytes = json_serialize(StopProviderCommand {
            host_id,
            provider_id: parse_identifier(&IdentifierKind::ComponentId, provider_id)?,
            annotations,
        })?;

        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => Ok(json_deserialize(&msg.payload)?),
            Err(e) => Err(format!("Did not receive stop provider acknowledgement: {e}").into()),
        }
    }

    /// Issues a command to a specific host to perform a graceful termination. The target host will
    /// acknowledge receipt of the command before it attempts a shutdown. To deterministically
    /// verify that the host is down, a client should monitor for the "host stopped" event or
    /// passively detect the host down by way of a lack of heartbeat receipts
    #[instrument(level = "debug", skip_all)]
    pub async fn stop_host(
        &self,
        host_id: &str,
        timeout_ms: Option<u64>,
    ) -> Result<CtlOperationAck> {
        let host_id = parse_identifier(&IdentifierKind::HostId, host_id)?;
        let subject =
            broker::commands::stop_host(&self.topic_prefix, &self.lattice, host_id.as_str());
        debug!("stop_host:request {}", &subject);
        let bytes = json_serialize(StopHostCommand {
            host_id,
            timeout: timeout_ms,
        })?;

        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => Ok(json_deserialize(&msg.payload)?),
            Err(e) => Err(format!("Did not receive stop host acknowledgement: {e}").into()),
        }
    }

    async fn publish_and_wait<D: DeserializeOwned>(
        &self,
        subject: String,
        payload: Vec<u8>,
    ) -> Result<Vec<D>> {
        let reply = self.nc.new_inbox();
        let sub = self.nc.subscribe(reply.clone()).await?;
        self.nc
            .publish_with_reply_and_headers(
                subject.clone(),
                reply,
                otel::HeaderInjector::default_with_span().into(),
                payload.into(),
            )
            .await?;
        let nc = self.nc.clone();
        tokio::spawn(async move {
            if let Err(error) = nc.flush().await {
                error!(%error, "flush after publish");
            }
        });
        Ok(collect_sub_timeout::<D>(sub, self.auction_timeout, subject.as_str()).await)
    }

    /// Returns the receiver end of a channel that subscribes to the lattice event stream.
    /// Any [`Event`](struct@Event)s that are published after this channel is created
    /// will be added to the receiver channel's buffer, which can be observed or handled if needed.
    /// See the example for how you could use this receiver to handle events.
    ///
    /// # Example
    /// ```rust
    /// use wasmcloud_control_interface::{Client, ClientBuilder};
    /// async {
    ///   let nc = async_nats::connect("127.0.0.1:4222").await.unwrap();
    ///   let client = ClientBuilder::new(nc)
    ///                 .rpc_timeout(std::time::Duration::from_millis(1000))
    ///                 .auction_timeout(std::time::Duration::from_millis(1000))
    ///                 .build();    
    ///   let mut receiver = client.events_receiver("actor_scaled").await.unwrap();
    ///   while let Some(evt) = receiver.recv().await {
    ///       println!("Event received: {:?}", evt);
    ///   }
    /// };
    /// ```
    #[allow(clippy::missing_errors_doc)] // TODO: Document errors
    pub async fn events_receiver(&self, event_types: Vec<String>) -> Result<Receiver<Event>> {
        let (sender, receiver) = tokio::sync::mpsc::channel(5000);
        let futs = event_types.into_iter().map(|event_type| {
            self.nc
                .subscribe(format!("wasmbus.evt.{}.{}", self.lattice, event_type))
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error + Send + Sync>)
        });
        let subs: Vec<Subscriber> = futures::future::join_all(futs)
            .await
            .into_iter()
            .collect::<Result<_>>()?;
        let mut stream = futures::stream::select_all(subs);
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let Ok(evt) = json_deserialize::<Event>(&msg.payload) else {
                    error!("Object received on event stream was not a CloudEvent");
                    continue;
                };
                trace!("received event: {:?}", evt);
                let Ok(()) = sender.send(evt).await else {
                    break;
                };
            }
        });
        Ok(receiver)
    }
}

/// Helper function that serializes the data and maps the error
fn json_serialize<T>(item: T) -> Result<Vec<u8>>
where
    T: Serialize,
{
    serde_json::to_vec(&item).map_err(|e| format!("JSON serialization failure: {e}").into())
}

/// Helper function that deserializes the data and maps the error
fn json_deserialize<'de, T: Deserialize<'de>>(buf: &'de [u8]) -> Result<T> {
    serde_json::from_slice(buf).map_err(|e| format!("JSON deserialization failure: {e}").into())
}

/// Collect results until timeout has elapsed
pub async fn collect_sub_timeout<T: DeserializeOwned>(
    mut sub: async_nats::Subscriber,
    timeout: Duration,
    reason: &str,
) -> Vec<T> {
    let mut items = Vec::new();
    let sleep = tokio::time::sleep(timeout);
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            msg = sub.next() => {
                let Some(msg) = msg else {
                    break;
                };
                if msg.payload.is_empty() {
                    break;
                }
                match json_deserialize::<T>(&msg.payload) {
                    Ok(item) => items.push(item),
                    Err(error) => {
                        error!(%reason, %error,
                            "deserialization error in auction - results may be incomplete",
                        );
                        break;
                    }
                }
            },
            () = &mut sleep => { /* timeout */ break; }
        }
    }
    items
}

enum IdentifierKind {
    HostId,
    ComponentId,
    ActorRef,
    ProviderRef,
    LinkName,
}

fn assert_non_empty_string(input: &str, message: &str) -> Result<String> {
    if input.trim().is_empty() {
        Err(message.into())
    } else {
        Ok(input.trim().to_string())
    }
}

//NOTE(ahmedtadde): For an initial implementation, we just want to make sure that the identifier is, at very least, not an empty string.
//This parser should be refined over time as needed.
fn parse_identifier<T: AsRef<str>>(kind: &IdentifierKind, value: T) -> Result<String> {
    let value = value.as_ref();
    match kind {
        IdentifierKind::HostId => assert_non_empty_string(value, "Host ID cannot be empty"),
        IdentifierKind::ComponentId => {
            assert_non_empty_string(value, "Component ID cannot be empty")
        }
        IdentifierKind::ActorRef => {
            assert_non_empty_string(value, "Actor OCI reference cannot be empty")
        }
        IdentifierKind::ProviderRef => {
            assert_non_empty_string(value, "Provider OCI reference cannot be empty")
        }
        IdentifierKind::LinkName => assert_non_empty_string(value, "Link Name cannot be empty"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Note: This test is a means of manually watching the event stream as CloudEvents are received
    /// It does not assert functionality, and so we've marked it as ignore to ensure it's not run by default
    /// It currently listens for 120 seconds then exits
    #[tokio::test]
    #[ignore]
    async fn test_events_receiver() {
        let nc = async_nats::connect("127.0.0.1:4222").await.unwrap();
        let client = ClientBuilder::new(nc)
            .timeout(Duration::from_millis(1000))
            .auction_timeout(Duration::from_millis(1000))
            .build();
        let mut receiver = client
            .events_receiver(vec!["foobar".to_string()])
            .await
            .unwrap();
        tokio::spawn(async move {
            while let Some(evt) = receiver.recv().await {
                println!("Event received: {evt:?}");
            }
        });
        println!("Listening to Cloud Events for 120 seconds. Then we will quit.");
        tokio::time::sleep(Duration::from_secs(120)).await;
    }

    #[test]
    fn test_parse_identifier() -> Result<()> {
        assert!(parse_identifier(&IdentifierKind::HostId, "").is_err());
        assert!(parse_identifier(&IdentifierKind::HostId, " ").is_err());
        let host_id = parse_identifier(&IdentifierKind::HostId, "             ");
        assert!(host_id.is_err(), "parsing host id should have failed");
        assert!(host_id
            .unwrap_err()
            .to_string()
            .contains("Host ID cannot be empty"));
        let provider_ref = parse_identifier(&IdentifierKind::ProviderRef, "");
        assert!(
            provider_ref.is_err(),
            "parsing provider ref should have failed"
        );
        assert!(provider_ref
            .unwrap_err()
            .to_string()
            .contains("Provider OCI reference cannot be empty"));
        assert!(parse_identifier(&IdentifierKind::HostId, "host_id").is_ok());
        let actor_id = parse_identifier(&IdentifierKind::ComponentId, "            iambatman  ")?;
        assert_eq!(actor_id, "iambatman");

        Ok(())
    }
}
