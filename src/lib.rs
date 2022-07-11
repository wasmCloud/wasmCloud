pub mod broker;
pub use wasmcloud_interface_lattice_control::*;
mod sub_stream;

use cloudevents::event::Event;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};
use sub_stream::collect_timeout;
use tokio::sync::mpsc::Receiver;
use tracing::{debug, error, instrument, trace};
use tracing_futures::Instrument;
use wasmbus_rpc::core::LinkDefinition;
use wasmbus_rpc::otel::OtelHeaderInjector;

type Result<T> = ::std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Lattice control interface client
#[derive(Clone)]
pub struct Client {
    nc: async_nats::Client,
    nsprefix: Option<String>,
    timeout: Duration,
    auction_timeout: Duration,
}

impl Client {
    /// Creates a new lattice control interface client
    pub fn new(
        nc: async_nats::Client,
        nsprefix: Option<String>,
        timeout: Duration,
        auction_timeout: Duration,
    ) -> Self {
        Client {
            nc,
            nsprefix,
            timeout,
            auction_timeout,
        }
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
                OtelHeaderInjector::default_with_span().into(),
                payload.into(),
            ),
        )
        .await
        {
            Err(_) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out").into()),
            Ok(Ok(message)) => Ok(message),
            Ok(Err(e)) => Err(e),
        }
    }

    /// Queries the lattice for all responsive hosts, waiting for the full period specified by _timeout_.
    #[instrument(level = "debug", skip_all)]
    pub async fn get_hosts(&self) -> Result<Vec<Host>> {
        let subject = broker::queries::hosts(&self.nsprefix);
        debug!("get_hosts:publish {}", &subject);
        let reply = self.nc.new_inbox();
        let sub = self.nc.subscribe(reply.clone()).await?;
        self.nc
            .publish_with_reply_and_headers(
                subject,
                reply,
                OtelHeaderInjector::default_with_span().into(),
                Vec::new().into(),
            )
            .await?;
        Ok(collect_timeout::<Host>(sub, self.auction_timeout, "hosts").await)
    }

    /// Retrieves the contents of a running host
    #[instrument(level = "debug", skip_all)]
    pub async fn get_host_inventory(&self, host_id: &str) -> Result<HostInventory> {
        let subject = broker::queries::host_inventory(&self.nsprefix, host_id);
        debug!("get_host_inventory:request {}", &subject);
        match self.request_timeout(subject, vec![], self.timeout).await {
            Ok(msg) => {
                let hi: HostInventory = json_deserialize(&msg.payload)?;
                Ok(hi)
            }
            Err(e) => Err(format!("Did not receive host inventory from target host: {}", e).into()),
        }
    }

    /// Retrieves the full set of all cached claims in the lattice by getting a response from the first
    /// host that answers this query
    #[instrument(level = "debug", skip_all)]
    pub async fn get_claims(&self) -> Result<GetClaimsResponse> {
        let subject = broker::queries::claims(&self.nsprefix);
        debug!("get_claims:request {}", &subject);
        match self.request_timeout(subject, vec![], self.timeout).await {
            Ok(msg) => {
                let list: GetClaimsResponse = json_deserialize(&msg.payload)?;
                Ok(list)
            }
            Err(e) => Err(format!("Did not receive claims from lattice: {}", e).into()),
        }
    }

    /// Performs an actor auction within the lattice, publishing a set of constraints and the metadata for the actor
    /// in question. This will always wait for the full period specified by _duration_, and then return the set of
    /// gathered results. It is then up to the client to choose from among the "auction winners" to issue the appropriate
    /// command to start an actor. Clients cannot assume that auctions will always return at least one result.
    #[instrument(level = "debug", skip_all)]
    pub async fn perform_actor_auction(
        &self,
        actor_ref: &str,
        constraints: HashMap<String, String>,
    ) -> Result<Vec<ActorAuctionAck>> {
        let subject = broker::actor_auction_subject(&self.nsprefix);
        let bytes = json_serialize(ActorAuctionRequest {
            actor_ref: actor_ref.to_string(),
            constraints,
        })?;
        debug!("actor_auction:publish {}", &subject);
        let reply = self.nc.new_inbox();
        let sub = self.nc.subscribe(reply.clone()).await?;
        self.nc
            .publish_with_reply_and_headers(
                subject,
                reply,
                OtelHeaderInjector::default_with_span().into(),
                bytes.into(),
            )
            .await?;
        Ok(collect_timeout(sub, self.auction_timeout, "actor").await)
    }

    /// Performs a provider auction within the lattice, publishing a set of constraints and the metadata for the provider
    /// in question. This will always wait for the full period specified by _duration_, and then return the set of gathered
    /// results. It is then up to the client to choose from among the "auction winners" and issue the appropriate command
    /// to start a provider. Clients cannot assume that auctions will always return at least one result.
    #[instrument(level = "debug", skip_all)]
    pub async fn perform_provider_auction(
        &self,
        provider_ref: &str,
        link_name: &str,
        constraints: HashMap<String, String>,
    ) -> Result<Vec<ProviderAuctionAck>> {
        let subject = broker::provider_auction_subject(&self.nsprefix);
        let bytes = json_serialize(ProviderAuctionRequest {
            provider_ref: provider_ref.to_string(),
            link_name: link_name.to_string(),
            constraints,
        })?;
        debug!("provider_auction:publish {}", &subject);
        let reply = self.nc.new_inbox();
        let sub = self.nc.subscribe(reply.clone()).await?;
        self.nc
            .publish_with_reply_and_headers(
                subject,
                reply,
                OtelHeaderInjector::default_with_span().into(),
                bytes.into(),
            )
            .await?;
        Ok(collect_timeout(sub, self.auction_timeout, "provider").await)
    }

    /// Sends a request to the given host to start a given actor by its OCI reference. This returns an acknowledgement
    /// of _receipt_ of the command, not a confirmation that the actor started. An acknowledgement will either indicate
    /// some form of validation failure, or, if no failure occurs, the receipt of the command. To avoid blocking consumers,
    /// wasmCloud hosts will acknowledge the start actor command prior to fetching the actor's OCI bytes. If a client needs
    /// deterministic results as to whether the actor completed its startup process, the client will have to monitor
    /// the appropriate event in the control event stream
    #[instrument(level = "debug", skip_all)]
    pub async fn start_actor(
        &self,
        host_id: &str,
        actor_ref: &str,
        count: u16,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::commands::start_actor(&self.nsprefix, host_id);
        debug!("start_actor:request {}", &subject);
        let bytes = json_serialize(StartActorCommand {
            count,
            actor_ref: actor_ref.to_string(),
            host_id: host_id.to_string(),
            annotations,
        })?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.payload)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive start actor acknowledgement: {}", e).into()),
        }
    }

    /// Sends a request to the given host to scale a given actor. This returns an acknowledgement of _receipt_ of the
    /// command, not a confirmation that the actor scaled. An acknowledgement will either indicate some form of
    /// validation failure, or, if no failure occurs, the receipt of the command. To avoid blocking consumers,
    /// wasmCloud hosts will acknowledge the scale actor command prior to fetching the actor's OCI bytes. If a client
    /// needs deterministic results as to whether the actor completed its startup process, the client will have to
    /// monitor the appropriate event in the control event stream
    #[instrument(level = "debug", skip_all)]
    pub async fn scale_actor(
        &self,
        host_id: &str,
        actor_ref: &str,
        actor_id: &str,
        count: u16,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::commands::scale_actor(&self.nsprefix, host_id);
        debug!("scale_actor:request {}", &subject);
        let bytes = json_serialize(ScaleActorCommand {
            count,
            actor_ref: actor_ref.to_string(),
            host_id: host_id.to_string(),
            actor_id: actor_id.to_string(),
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

    /// Publishes a registry credential map to the control interface of the lattice.
    /// All hosts will be listening and all will overwrite their registry credential
    /// map with the new information. It is highly recommended you use TLS connections
    /// with NATS and isolate the control interface credentials when using this
    /// function in production as the data contains secrets
    #[instrument(level = "debug", skip_all)]
    pub async fn put_registries(&self, registries: RegistryCredentialMap) -> Result<()> {
        let subject = broker::publish_registries(&self.nsprefix);
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

    /// Publishes the link advertisement message to the lattice that is published when code invokes the `set_link`
    /// function on a `Host` struct instance. This operation pushes to a queue-subscribed topic, and therefore
    /// awaits confirmation from the single psuedo-randomly chosen recipient host. If that one host fails to acknowledge,
    /// or if no hosts acknowledge within the timeout period, this operation is considered a failure
    #[instrument(level = "debug", skip_all)]
    pub async fn advertise_link(
        &self,
        actor_id: &str,
        provider_id: &str,
        contract_id: &str,
        link_name: &str,
        values: HashMap<String, String>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::advertise_link(&self.nsprefix);
        debug!("advertise_link:request {}", &subject);
        let mut ld = LinkDefinition::default();
        ld.actor_id = actor_id.to_string();
        ld.provider_id = provider_id.to_string();
        ld.contract_id = contract_id.to_string();
        ld.link_name = link_name.to_string();
        ld.values = values;
        let bytes = crate::json_serialize(&ld)?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.payload)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive advertise link acknowledgement: {}", e).into()),
        }
    }

    /// Publishes a request to remove a link definition to the lattice.
    #[instrument(level = "debug", skip_all)]
    pub async fn remove_link(
        &self,
        actor_id: &str,
        contract_id: &str,
        link_name: &str,
    ) -> Result<CtlOperationAck> {
        let subject = broker::remove_link(&self.nsprefix);
        debug!("remove_link:request {}", &subject);
        let mut ld = LinkDefinition::default();
        ld.actor_id = actor_id.to_string();
        ld.contract_id = contract_id.to_string();
        ld.link_name = link_name.to_string();
        let bytes = crate::json_serialize(&ld)?;
        match self.request_timeout(subject, bytes, self.timeout).await {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.payload)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive remove link acknowledgement: {}", e).into()),
        }
    }

    /// Publishes a request to retrieve all current link definitions.
    #[instrument(level = "debug", skip_all)]
    pub async fn query_links(&self) -> Result<LinkDefinitionList> {
        let subject = broker::queries::link_definitions(&self.nsprefix);
        debug!("query_links:request {}", &subject);
        match self.request_timeout(subject, vec![], self.timeout).await {
            Ok(msg) => json_deserialize(&msg.payload),
            Err(e) => Err(format!("Did not receive a response to links query: {}", e).into()),
        }
    }

    /// Issue a command to a host instructing that it replace an existing actor (indicated by its
    /// public key) with a new actor indicated by an OCI image reference. The host will acknowledge
    /// this request as soon as it verifies that the target actor is running. This acknowledgement
    /// occurs **before** the new bytes are downloaded. Live-updating an actor can take a long
    /// time and control clients cannot block waiting for a reply that could come several seconds
    /// later. If you need to verify that the actor has been updated, you will want to set up a
    /// listener for the appropriate **PublishedEvent** which will be published on the control events
    /// channel in JSON
    #[instrument(level = "debug", skip_all)]
    pub async fn update_actor(
        &self,
        host_id: &str,
        existing_actor_id: &str,
        new_actor_ref: &str,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::commands::update_actor(&self.nsprefix, host_id);
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

    /// Issues a command to a host to start a provider with a given OCI reference using the specified link
    /// name (or "default" if none is specified). The target wasmCloud host will acknowledge the receipt
    /// of this command _before_ downloading the provider's bytes from the OCI registry, indicating either
    /// a validation failure or success. If a client needs deterministic guarantees that the provider has
    /// completed its startup process, such a client needs to monitor the control event stream for the
    /// appropriate event. If a host ID is not supplied (empty string), then this function will return
    /// an early acknowledgement, go find a host, and then submit the start request to a target host.
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
                &self.nsprefix,
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
                        &this.nsprefix,
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

    /// Issues a command to a host to stop a provider for the given OCI reference, link name, and contract ID. The
    /// target wasmCloud host will acknowledge the receipt of this command, and _will not_ supply a discrete
    /// confirmation that a provider has terminated. For that kind of information, the client must also monitor
    /// the control event stream
    #[instrument(level = "debug", skip_all)]
    pub async fn stop_provider(
        &self,
        host_id: &str,
        provider_ref: &str,
        link_name: &str,
        contract_id: &str,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::commands::stop_provider(&self.nsprefix, host_id);
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

    /// Issues a command to a host to stop an actor for the given OCI reference. The
    /// target wasmCloud host will acknowledge the receipt of this command, and _will not_ supply a discrete
    /// confirmation that the actor has terminated. For that kind of information, the client must also monitor
    /// the control event stream
    #[instrument(level = "debug", skip_all)]
    pub async fn stop_actor(
        &self,
        host_id: &str,
        actor_ref: &str,
        count: u16,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::commands::stop_actor(&self.nsprefix, host_id);
        debug!("stop_actor:request {}", &subject);
        let bytes = json_serialize(StopActorCommand {
            host_id: host_id.to_string(),
            actor_ref: actor_ref.to_string(),
            count,
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

    /// Issues a command to a specific host to perform a graceful termination. The target host
    /// will acknowledge receipt of the command before it attempts a shutdown. To deterministically
    /// verify that the host is down, a client should monitor for the "host stopped" event or
    /// passively detect the host down by way of a lack of heartbeat receipts
    #[instrument(level = "debug", skip_all)]
    pub async fn stop_host(
        &self,
        host_id: &str,
        timeout_ms: Option<u64>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::commands::stop_host(&self.nsprefix, host_id);
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

    /// Returns the receiver end of a channel that subscribes to the lattice control event stream.
    /// Any [`Event`](struct@Event)s that are published after this channel is created
    /// will be added to the receiver channel's buffer, which can be observed or handled if needed.
    /// See the example for how you could use this receiver to handle events.
    ///
    /// # Example
    /// ```rust
    /// use wasmcloud_control_interface::Client;
    /// async {
    ///   let nc = async_nats::connect("127.0.0.1:4222").await.unwrap();
    ///   let client = Client::new(nc, None, std::time::Duration::from_millis(1000),
    ///                        std::time::Duration::from_millis(1000));
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
    /// use wasmcloud_control_interface::Client;
    /// async {
    ///   let nc = async_nats::connect("0.0.0.0:4222").await.unwrap();
    ///   let client = Client::new(nc, None, std::time::Duration::from_millis(1000),
    ///                   std::time::Duration::from_millis(1000));
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
            .subscribe(broker::control_event(&self.nsprefix))
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
    nsprefix: &Option<String>,
    timeout: Duration,
    host_id: &str,
    provider_ref: &str,
    link_name: Option<String>,
    annotations: Option<HashMap<String, String>>,
    provider_configuration: Option<String>,
) -> Result<CtlOperationAck> {
    let subject = broker::commands::start_provider(nsprefix, host_id);
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

    /// Note: This test is a means of manually watching the event stream as CloudEvents are received
    /// It does not assert functionality, and so we've marked it as ignore to ensure it's not run by default
    /// It currently listens for 120 seconds then exits
    #[tokio::test]
    #[ignore]
    async fn test_events_receiver() {
        let nc = async_nats::connect("127.0.0.1:4222").await.unwrap();
        let client = Client::new(
            nc,
            None,
            std::time::Duration::from_millis(1000),
            std::time::Duration::from_millis(1000),
        );
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
