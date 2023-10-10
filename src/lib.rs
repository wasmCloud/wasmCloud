//! # Control Interface Client
//!
//! This library provides a client API for consuming the wasmCloud control interface over a
//! NATS connection. This library can be used by multiple types of tools, and is also used
//! by the control interface capability provider and the wash CLI
use std::fmt::Debug;
use std::marker::PhantomData;
use std::{collections::HashMap, time::Duration};

use cloudevents::event::Event;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sub_stream::collect_timeout;
use tokio::sync::mpsc::Receiver;
use tracing::{debug, error, instrument, trace};
use tracing_futures::Instrument;

mod broker;
pub mod kv;
mod otel;
mod sub_stream;
mod types;

use kv::{Build, CachedKvStore, DirectKvStore};
pub use types::*;

use crate::kv::KvStore;
use crate::otel::OtelHeaderInjector;

type Result<T> = ::std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Lattice control interface client
#[derive(Clone)]
pub struct Client<T: Clone> {
    nc: async_nats::Client,
    topic_prefix: Option<String>,
    pub lattice_prefix: String,
    timeout: Duration,
    auction_timeout: Duration,
    kvstore: T,
}

impl<T: Clone> Debug for Client<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("topic_prefix", &self.topic_prefix)
            .field("lattice_prefix", &self.lattice_prefix)
            .field("timeout", &self.timeout)
            .field("auction_timeout", &self.auction_timeout)
            .finish()
    }
}

/// A client builder that can be used to fluently provide configuration settings used to construct
/// the control interface client
pub struct ClientBuilder<T> {
    nc: Option<async_nats::Client>,
    topic_prefix: Option<String>,
    lattice_prefix: String,
    timeout: Duration,
    auction_timeout: Duration,
    js_domain: Option<String>,
    store_placeholder: PhantomData<T>,
}

impl<T> Default for ClientBuilder<T> {
    fn default() -> Self {
        Self {
            nc: None,
            topic_prefix: None,
            lattice_prefix: "default".to_string(),
            timeout: Duration::from_secs(2),
            auction_timeout: Duration::from_secs(5),
            js_domain: None,
            store_placeholder: PhantomData,
        }
    }
}

impl ClientBuilder<DirectKvStore> {
    /// Creates a new client builder using the given client, set up to use the
    /// [`DirectKvStore`](DirectKvStore) for bucket operations
    pub fn new(nc: async_nats::Client) -> ClientBuilder<DirectKvStore> {
        ClientBuilder {
            nc: Some(nc),
            ..Default::default()
        }
    }
}

impl ClientBuilder<CachedKvStore> {
    /// Creates a new client builder using the given client, set up to use the
    /// [`CachedKvStore`](CachedKvStore) for bucket operations
    pub fn new_caching(nc: async_nats::Client) -> ClientBuilder<CachedKvStore> {
        ClientBuilder {
            nc: Some(nc),
            ..Default::default()
        }
    }
}

impl<T: KvStore + Build + Clone> ClientBuilder<T> {
    /// Creates a new client builder using the given client, set up to use the generic key value
    /// store type `T` for bucket operations. This is useful for times when you need to specify a
    /// type dynamically
    ///
    /// ```no_run
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// use wasmcloud_control_interface::{kv::CachedKvStore, ClientBuilder};
    ///
    /// let nc = async_nats::connect("localhost:4222").await.unwrap();
    /// let client = ClientBuilder::<CachedKvStore>::new_generic(nc.clone()).build().await.unwrap();
    /// // or
    /// let builder: ClientBuilder<CachedKvStore> = ClientBuilder::new_generic(nc);
    /// let client = builder.build().await.unwrap();
    /// # }
    /// ```
    ///
    /// This function is not just `new` because we want to preserve the default behavior of the
    /// direct KV store behind the already used `new` function.
    pub fn new_generic(nc: async_nats::Client) -> ClientBuilder<T> {
        ClientBuilder {
            nc: Some(nc),
            ..Default::default()
        }
    }

    /// Completes the generation of a control interface client. This function is async because it
    /// will attempt to locate and attach to a metadata key-value bucket (`LATTICEDATA_{prefix}`)
    /// when starting. If this bucket doesn't exist (meaning no hosts have actually run in that
    /// lattice), than an error will be returned
    pub async fn build(self) -> Result<Client<T>> {
        if let Some(nc) = self.nc {
            let kvstore = T::build(nc.clone(), &self.lattice_prefix, self.js_domain).await?;
            Ok(Client {
                nc,
                topic_prefix: self.topic_prefix,
                lattice_prefix: self.lattice_prefix,
                timeout: self.timeout,
                auction_timeout: self.auction_timeout,
                kvstore,
            })
        } else {
            Err("Cannot create a control interface client without a NATS client".into())
        }
    }
}

impl<T> ClientBuilder<T> {
    /// Sets the topic prefix for the NATS topic used for all control requests. Not to be confused
    /// with lattice ID/prefix
    pub fn topic_prefix(self, prefix: impl Into<String>) -> ClientBuilder<T> {
        ClientBuilder {
            topic_prefix: Some(prefix.into()),
            ..self
        }
    }

    /// The lattice ID/prefix used for this client. If this function is not invoked, the prefix will
    /// be set to `default`
    pub fn lattice_prefix(self, prefix: impl Into<String>) -> ClientBuilder<T> {
        ClientBuilder {
            lattice_prefix: prefix.into(),
            ..self
        }
    }

    /// Sets the timeout for standard calls and RPC invocations used by the client. If not set, the
    /// default will be 2 seconds
    #[deprecated(since = "0.30.0", note = "please use `timeout` instead")]
    pub fn rpc_timeout(self, timeout: Duration) -> ClientBuilder<T> {
        ClientBuilder { timeout, ..self }
    }

    /// Sets the timeout for control interface requests issued by the client. If not set, the
    /// default will be 2 seconds
    pub fn timeout(self, timeout: Duration) -> ClientBuilder<T> {
        ClientBuilder { timeout, ..self }
    }

    /// Sets the timeout for auction (scatter/gather) operations. If not set, the default will be 5
    /// seconds
    pub fn auction_timeout(self, timeout: Duration) -> ClientBuilder<T> {
        ClientBuilder {
            auction_timeout: timeout,
            ..self
        }
    }

    /// Sets the JetStream domain for this client, which can be critical for locating the right
    /// key-value bucket for lattice metadata storage. If this is skipped, then the JS domain will
    /// be `None`
    pub fn js_domain(self, domain: impl Into<String>) -> ClientBuilder<T> {
        ClientBuilder {
            js_domain: Some(domain.into()),
            ..self
        }
    }
}

impl<T: KvStore + Clone + Send + Sync> Client<T> {
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
                OtelHeaderInjector::default_with_span().into(),
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

    /// Returns a handle to the underlying metadata client for use in advanced scenarios and queries
    pub fn lattice_metadata_client(&self) -> &T {
        &self.kvstore
    }

    /// Queries the lattice for all responsive hosts, waiting for the full period specified by
    /// _timeout_.
    #[instrument(level = "debug", skip_all)]
    pub async fn get_hosts(&self) -> Result<Vec<Host>> {
        let subject = broker::queries::hosts(&self.topic_prefix, &self.lattice_prefix);
        debug!("get_hosts:publish {}", &subject);
        self.publish_and_wait(subject, Vec::new()).await
    }

    /// Retrieves the contents of a running host
    #[instrument(level = "debug", skip_all)]
    pub async fn get_host_inventory(&self, host_id: &str) -> Result<HostInventory> {
        let subject =
            broker::queries::host_inventory(&self.topic_prefix, &self.lattice_prefix, host_id);
        debug!("get_host_inventory:request {}", &subject);
        match self.request_timeout(subject, vec![], self.timeout).await {
            Ok(msg) => {
                let hi: HostInventory = json_deserialize(&msg.payload)?;
                Ok(hi)
            }
            Err(e) => Err(format!("Did not receive host inventory from target host: {}", e).into()),
        }
    }

    /// Retrieves the full set of all cached claims in the lattice.   
    #[instrument(level = "debug", skip_all)]
    pub async fn get_claims(&self) -> Result<Vec<HashMap<String, String>>> {
        self.kvstore.get_all_claims().await
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
        constraints: HashMap<String, String>,
    ) -> Result<Vec<ActorAuctionAck>> {
        let subject = broker::actor_auction_subject(&self.topic_prefix, &self.lattice_prefix);
        let bytes = json_serialize(ActorAuctionRequest {
            actor_ref: actor_ref.to_string(),
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
        link_name: &str,
        constraints: HashMap<String, String>,
    ) -> Result<Vec<ProviderAuctionAck>> {
        let subject = broker::provider_auction_subject(&self.topic_prefix, &self.lattice_prefix);
        let bytes = json_serialize(ProviderAuctionRequest {
            provider_ref: provider_ref.to_string(),
            link_name: link_name.to_string(),
            constraints,
        })?;
        debug!("provider_auction:publish {}", &subject);
        self.publish_and_wait(subject, bytes).await
    }

    /// Sends a request to the given host to start a given actor by its OCI reference. This returns
    /// an acknowledgement of _receipt_ of the command, not a confirmation that the actor started.
    /// An acknowledgement will either indicate some form of validation failure, or, if no failure
    /// occurs, the receipt of the command. To avoid blocking consumers, wasmCloud hosts will
    /// acknowledge the start actor command prior to fetching the actor's OCI bytes. If a client
    /// needs deterministic results as to whether the actor completed its startup process, the
    /// client will have to monitor the appropriate event in the control event stream
    #[instrument(level = "debug", skip_all)]
    #[deprecated(since = "0.30.0", note = "please use `scale_actor` instead")]
    pub async fn start_actor(
        &self,
        host_id: &str,
        actor_ref: &str,
        count: u16,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        // It makes no logical sense to start 0 actors, so we represent that as an unbounded max instead.
        let max = if count == 0 { None } else { Some(count) };
        self.scale_actor(host_id, actor_ref, max, annotations).await
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
    /// `max_concurrent`: The maximum number of requests this actor handle run concurrently. `None` represents an unbounded
    /// level of concurrency while `0` will stop the actor.
    /// `annotations`: Optional annotations to apply to the actor
    #[instrument(level = "debug", skip_all)]
    pub async fn scale_actor(
        &self,
        host_id: &str,
        actor_ref: &str,
        max_concurrent: Option<u16>,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject =
            broker::commands::scale_actor(&self.topic_prefix, &self.lattice_prefix, host_id);
        debug!("scale_actor:request {}", &subject);
        let bytes = json_serialize(ScaleActorCommand {
            max_concurrent,
            actor_ref: actor_ref.to_string(),
            host_id: host_id.to_string(),
            annotations,
        })?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.payload)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive scale actor acknowledgement: {}", e).into()),
        }
    }

    /// Publishes a registry credential map to the control interface of the lattice. All hosts will
    /// be listening and all will overwrite their registry credential map with the new information.
    /// It is highly recommended you use TLS connections with NATS and isolate the control interface
    /// credentials when using this function in production as the data contains secrets
    #[instrument(level = "debug", skip_all)]
    pub async fn put_registries(&self, registries: RegistryCredentialMap) -> Result<()> {
        let subject = broker::publish_registries(&self.topic_prefix, &self.lattice_prefix);
        debug!("put_registries:publish {}", &subject);
        let bytes = json_serialize(&registries)?;
        let resp = self
            .nc
            .publish_with_headers(
                subject,
                OtelHeaderInjector::default_with_span().into(),
                bytes.into(),
            )
            .await;
        if let Err(e) = resp {
            Err(format!("Failed to push registry credential map: {}", e).into())
        } else {
            Ok(())
        }
    }

    /// Puts a link into the lattice metadata keyvalue bucket. Returns an error if it was unable to
    /// put the link
    #[instrument(level = "debug", skip_all)]
    pub async fn advertise_link(
        &self,
        actor_id: &str,
        provider_id: &str,
        contract_id: &str,
        link_name: &str,
        values: HashMap<String, String>,
    ) -> Result<()> {
        self.kvstore
            .put_link(LinkDefinition {
                actor_id: actor_id.to_string(),
                provider_id: provider_id.to_string(),
                contract_id: contract_id.to_string(),
                link_name: link_name.to_string(),
                values,
            })
            .await
    }

    /// Removes a link from the lattice metadata keyvalue bucket. Returns an error if it was unable
    /// to delete. This is an idempotent operation.
    #[instrument(level = "debug", skip_all)]
    pub async fn remove_link(
        &self,
        actor_id: &str,
        contract_id: &str,
        link_name: &str,
    ) -> Result<()> {
        self.kvstore
            .delete_link(actor_id, contract_id, link_name)
            .await
    }

    /// Retrieves the list of link definitions stored in the lattice metadata key-value bucket. If
    /// the client was created with caching, this will return the cached list of links. Otherwise,
    /// it will query the bucket for the list of links.
    #[instrument(level = "debug", skip_all)]
    pub async fn query_links(&self) -> Result<Vec<LinkDefinition>> {
        self.kvstore.get_links().await
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
        let subject =
            broker::commands::update_actor(&self.topic_prefix, &self.lattice_prefix, host_id);
        debug!("update_actor:request {}", &subject);
        let bytes = json_serialize(UpdateActorCommand {
            host_id: host_id.to_string(),
            actor_id: existing_actor_id.to_string(),
            new_actor_ref: new_actor_ref.to_string(),
            annotations,
        })?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.payload)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive update actor acknowledgement: {}", e).into()),
        }
    }

    /// Issues a command to a host to start a provider with a given OCI reference using the
    /// specified link name (or "default" if none is specified). The target wasmCloud host will
    /// acknowledge the receipt of this command _before_ downloading the provider's bytes from the
    /// OCI registry, indicating either a validation failure or success. If a client needs
    /// deterministic guarantees that the provider has completed its startup process, such a client
    /// needs to monitor the control event stream for the appropriate event. If a host ID is not
    /// supplied (empty string), then this function will return an early acknowledgement, go find a
    /// host, and then submit the start request to a target host.
    #[instrument(level = "debug", skip_all)]
    pub async fn start_provider(
        &self,
        host_id: &str,
        provider_ref: &str,
        link_name: Option<String>,
        annotations: Option<HashMap<String, String>>,
        provider_configuration: Option<String>,
    ) -> Result<CtlOperationAck> {
        let provider_ref = provider_ref.to_string();
        if !host_id.trim().is_empty() {
            start_provider_(
                &self.nc,
                &self.topic_prefix,
                &self.lattice_prefix,
                self.timeout,
                host_id,
                &provider_ref,
                link_name,
                annotations,
                provider_configuration,
            )
            .in_current_span()
            .await
        } else {
            // If a host isn't supplied, try to find one via auction.
            // If no host is found, return error.
            // If a host is found, start brackground request to start provider and return Ack
            let mut error = String::new();
            debug!("start_provider:deferred (no-host) request");
            let current_span = tracing::Span::current();
            let host = match self.get_hosts().await {
                Err(e) => {
                    error = format!("failed to query hosts for no-host provider start: {}", e);
                    None
                }
                Ok(hs) => hs.into_iter().next(),
            };
            if let Some(host) = host {
                let this = self.clone();
                tokio::spawn(async move {
                    let _ = start_provider_(
                        &this.nc,
                        &this.topic_prefix,
                        &this.lattice_prefix,
                        this.timeout,
                        &host.id,
                        &provider_ref,
                        link_name,
                        annotations,
                        provider_configuration,
                    )
                    .instrument(current_span)
                    .await;
                });
            } else if error.is_empty() {
                error = "No hosts detected in in no-host provider start.".to_string();
            }
            if !error.is_empty() {
                error!("{}", error);
            }
            Ok(CtlOperationAck {
                accepted: true,
                error,
            })
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
        provider_ref: &str,
        link_name: &str,
        contract_id: &str,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject =
            broker::commands::stop_provider(&self.topic_prefix, &self.lattice_prefix, host_id);
        debug!("stop_provider:request {}", &subject);
        let bytes = json_serialize(StopProviderCommand {
            host_id: host_id.to_string(),
            provider_ref: provider_ref.to_string(),
            link_name: link_name.to_string(),
            contract_id: contract_id.to_string(),
            annotations,
        })?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.payload)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive stop provider acknowledgement: {}", e).into()),
        }
    }

    /// Issues a command to a host to stop an actor for the given OCI reference. The target
    /// wasmCloud host will acknowledge the receipt of this command, and _will not_ supply a
    /// discrete confirmation that the actor has terminated. For that kind of information, the
    /// client must also monitor the control event stream
    #[instrument(level = "debug", skip_all)]
    pub async fn stop_actor(
        &self,
        host_id: &str,
        actor_ref: &str,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject =
            broker::commands::stop_actor(&self.topic_prefix, &self.lattice_prefix, host_id);
        debug!("stop_actor:request {}", &subject);
        let bytes = json_serialize(StopActorCommand {
            host_id: host_id.to_string(),
            actor_ref: actor_ref.to_string(),
            annotations,
        })?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.payload)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive stop actor acknowledgement: {}", e).into()),
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
        let subject =
            broker::commands::stop_host(&self.topic_prefix, &self.lattice_prefix, host_id);
        debug!("stop_host:request {}", &subject);
        let bytes = json_serialize(StopHostCommand {
            host_id: host_id.to_owned(),
            timeout: timeout_ms,
        })?;

        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.payload)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive stop host acknowledgement: {}", e).into()),
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
                OtelHeaderInjector::default_with_span().into(),
                payload.into(),
            )
            .await?;
        let nc = self.nc.clone();
        tokio::spawn(async move {
            if let Err(error) = nc.flush().await {
                error!(%error, "flush after publish");
            }
        });
        Ok(collect_timeout::<D>(sub, self.auction_timeout, subject.as_str()).await)
    }

    /// Returns the receiver end of a channel that subscribes to the lattice control event stream.
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
    ///                 .build().await.unwrap();
    ///   let mut receiver = client.events_receiver().await.unwrap();
    ///   tokio::spawn( async move {
    ///       while let Some(evt) = receiver.recv().await {
    ///           println!("Event received: {:?}", evt);
    ///       }
    ///   });
    ///   // perform other operations on client
    ///   client.get_host_inventory("NAEXHW...").await.unwrap();
    /// };
    /// ```
    ///
    /// Once you're finished with the event receiver, be sure to call `drop` with the receiver
    /// as an argument. This closes the channel and will prevent the sender from endlessly
    /// sending messages into the channel buffer.
    ///
    /// # Example
    /// ```rust
    /// use wasmcloud_control_interface::{Client, ClientBuilder};
    /// async {
    ///   let nc = async_nats::connect("0.0.0.0:4222").await.unwrap();
    ///   let client = ClientBuilder::new(nc)
    ///                 .rpc_timeout(std::time::Duration::from_millis(1000))
    ///                 .auction_timeout(std::time::Duration::from_millis(1000))
    ///                 .build().await.unwrap();    
    ///   let mut receiver = client.events_receiver().await.unwrap();
    ///   // read the docs for flume receiver. You can use it in either sync or async code
    ///   // The receiver can be cloned() as needed.
    ///   // If you drop the receiver. The subscriber will exit
    ///   // If the nats connection ic closed, the loop below will exit.
    ///   while let Some(evt) = receiver.recv().await {
    ///       println!("Event received: {:?}", evt);
    ///   }
    /// };
    /// ```
    pub async fn events_receiver(&self) -> Result<Receiver<Event>> {
        use futures::StreamExt as _;
        let (sender, receiver) = tokio::sync::mpsc::channel(5000);
        let mut sub = self
            .nc
            .subscribe(broker::control_event(&self.lattice_prefix))
            .await?;
        tokio::spawn(async move {
            while let Some(msg) = sub.next().await {
                let evt = match json_deserialize::<Event>(&msg.payload) {
                    Ok(evt) => evt,
                    Err(_) => {
                        error!("Object received on event stream was not a CloudEvent");
                        continue;
                    }
                };
                trace!("received event: {:?}", evt);
                // If the channel is disconnected, stop sending events
                if sender.send(evt).await.is_err() {
                    let _ = sub.unsubscribe().await;
                    break;
                }
            }
        });
        Ok(receiver)
    }
}

// [ss]: renamed to json_serialize and json_deserialize to avoid confusion
//   with msgpack serialize and deserialize, used for rpc messages.
//
/// The standard function for serializing codec structs into a format that can be
/// used for message exchange between actor and host. Use of any other function to
/// serialize could result in breaking incompatibilities.
pub fn json_serialize<T>(
    item: T,
) -> ::std::result::Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>
where
    T: Serialize,
{
    serde_json::to_vec(&item).map_err(|e| format!("JSON serialization failure: {}", e).into())
}

/// The standard function for de-serializing codec structs from a format suitable
/// for message exchange between actor and host. Use of any other function to
/// deserialize could result in breaking incompatibilities.
pub fn json_deserialize<'de, T: Deserialize<'de>>(
    buf: &'de [u8],
) -> ::std::result::Result<T, Box<dyn std::error::Error + Send + Sync>> {
    serde_json::from_slice(buf).map_err(|e| {
        {
            std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("JSON deserialization failure: {}", e),
            )
        }
        .into()
    })
}

// "selfless" helper function that submits a start provider request to a host
#[allow(clippy::too_many_arguments)]
async fn start_provider_(
    client: &async_nats::Client,
    topic_prefix: &Option<String>,
    lattice_prefix: &str,
    timeout: Duration,
    host_id: &str,
    provider_ref: &str,
    link_name: Option<String>,
    annotations: Option<HashMap<String, String>>,
    provider_configuration: Option<String>,
) -> Result<CtlOperationAck> {
    let subject = broker::commands::start_provider(topic_prefix, lattice_prefix, host_id);
    debug!("start_provider:request {}", &subject);
    let bytes = json_serialize(StartProviderCommand {
        host_id: host_id.to_string(),
        provider_ref: provider_ref.to_string(),
        link_name: link_name.unwrap_or_else(|| "default".to_string()),
        annotations,
        configuration: provider_configuration,
    })?;
    match tokio::time::timeout(
        timeout,
        client.request_with_headers(
            subject,
            OtelHeaderInjector::default_with_span().into(),
            bytes.into(),
        ),
    )
    .await
    {
        Err(e) => Err(format!("Did not receive start provider acknowledgement: {}", e).into()),
        Ok(Err(e)) => Err(format!("Error sending or receiving message: {}", e).into()),
        Ok(Ok(msg)) => {
            let ack: CtlOperationAck = json_deserialize(&msg.payload)?;
            Ok(ack)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
            .build()
            .await
            .unwrap();
        let mut receiver = client.events_receiver().await.unwrap();
        tokio::spawn(async move {
            while let Some(evt) = receiver.recv().await {
                println!("Event received: {:?}", evt);
            }
        });
        println!("Listening to Cloud Events for 120 seconds. Then we will quit.");
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
    }
}
