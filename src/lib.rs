pub mod broker;
pub use wasmcloud_interface_lattice_control::*;
mod sub_stream;

use cloudevents::event::Event;
use crossbeam_channel::{unbounded, Receiver};
use futures::executor::block_on;
use log::{error, trace};
use nats::asynk::Connection;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};
use sub_stream::SubscriptionStream;
pub use wasmbus_rpc::{core::LinkDefinition, RpcClient};

type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error + Send + Sync>>;

/// Lattice control interface client
pub struct Client {
    nc: nats::asynk::Connection,
    nsprefix: Option<String>,
    timeout: Duration,
}

impl Client {
    /// Creates a new lattice control interface client
    pub fn new(nc: Connection, nsprefix: Option<String>, timeout: Duration) -> Self {
        Client {
            nc,
            nsprefix,
            timeout,
        }
    }

    /// Queries the lattice for all responsive hosts, waiting for the full period specified by _timeout_.
    pub async fn get_hosts(&self, timeout: Duration) -> Result<Vec<Host>> {
        let subject = broker::queries::hosts(&self.nsprefix);
        let sub = self.nc.request_multi(&subject, vec![]).await?;
        trace!("get_hosts: subscribing to {}", &subject);
        let hosts = SubscriptionStream::new(sub)
            .collect(timeout, "get hosts")
            .await;
        Ok(hosts)
    }

    /// Retrieves the contents of a running host
    pub async fn get_host_inventory(&self, host_id: &str) -> Result<HostInventory> {
        let subject = broker::queries::host_inventory(&self.nsprefix, host_id);
        trace!("get_host_inventory:request {}", &subject);
        match self
            .nc
            .request_timeout(&subject, vec![], self.timeout)
            .await
        {
            Ok(msg) => {
                let hi: HostInventory = json_deserialize(&msg.data)?;
                Ok(hi)
            }
            Err(e) => Err(format!("Did not receive host inventory from target host: {}", e).into()),
        }
    }

    /// Retrieves the full set of all cached claims in the lattice by getting a response from the first
    /// host that answers this query
    pub async fn get_claims(&self) -> Result<GetClaimsResponse> {
        let subject = broker::queries::claims(&self.nsprefix);
        trace!("get_claims:request {}", &subject);
        match self
            .nc
            .request_timeout(&subject, vec![], self.timeout)
            .await
        {
            Ok(msg) => {
                let list: GetClaimsResponse = json_deserialize(&msg.data)?;
                Ok(list)
            }
            Err(e) => Err(format!("Did not receive claims from lattice: {}", e).into()),
        }
    }

    /// Performs an actor auction within the lattice, publishing a set of constraints and the metadata for the actor
    /// in question. This will always wait for the full period specified by _duration_, and then return the set of
    /// gathered results. It is then up to the client to choose from among the "auction winners" to issue the appropriate
    /// command to start an actor. Clients cannot assume that auctions will always return at least one result.
    pub async fn perform_actor_auction(
        &self,
        actor_ref: &str,
        constraints: HashMap<String, String>,
        timeout: Duration,
    ) -> Result<Vec<ActorAuctionAck>> {
        let subject = broker::actor_auction_subject(&self.nsprefix);
        let bytes = json_serialize(ActorAuctionRequest {
            actor_ref: actor_ref.to_string(),
            constraints,
        })?;
        trace!("actor_auction: subscribing to {}", &subject);
        let sub = self.nc.request_multi(&subject, bytes).await?;
        let actors = SubscriptionStream::new(sub)
            .collect(timeout, "actor auction")
            .await;
        Ok(actors)
    }

    /// Performs a provider auction within the lattice, publishing a set of constraints and the metadata for the provider
    /// in question. This will always wait for the full period specified by _duration_, and then return the set of gathered
    /// results. It is then up to the client to choose from among the "auction winners" and issue the appropriate command
    /// to start a provider. Clients cannot assume that auctions will always return at least one result.
    pub async fn perform_provider_auction(
        &self,
        provider_ref: &str,
        link_name: &str,
        constraints: HashMap<String, String>,
        timeout: Duration,
    ) -> Result<Vec<ProviderAuctionAck>> {
        let subject = broker::provider_auction_subject(&self.nsprefix);
        let bytes = json_serialize(ProviderAuctionRequest {
            provider_ref: provider_ref.to_string(),
            link_name: link_name.to_string(),
            constraints,
        })?;
        trace!("provider_auction: subscribing to {}", &subject);
        let sub = self.nc.request_multi(&subject, bytes).await?;
        let providers = SubscriptionStream::new(sub)
            .collect(timeout, "provider auction")
            .await;
        Ok(providers)
    }

    /// Sends a request to the given host to start a given actor by its OCI reference. This returns an acknowledgement
    /// of _receipt_ of the command, not a confirmation that the actor started. An acknowledgement will either indicate
    /// some form of validation failure, or, if no failure occurs, the receipt of the command. To avoid blocking consumers,
    /// wasmCloud hosts will acknowledge the start actor command prior to fetching the actor's OCI bytes. If a client needs
    /// deterministic results as to whether the actor completed its startup process, the client will have to monitor
    /// the appropriate event in the control event stream
    pub async fn start_actor(
        &self,
        host_id: &str,
        actor_ref: &str,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::commands::start_actor(&self.nsprefix, host_id);
        trace!("start_actor:request {}", &subject);
        let bytes = json_serialize(StartActorCommand {
            actor_ref: actor_ref.to_string(),
            host_id: host_id.to_string(),
            annotations,
        })?;
        match self
            .nc
            .request_timeout(&subject, &bytes, self.timeout)
            .await
        {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.data)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive start actor acknowledgement: {}", e).into()),
        }
    }

    /// Publishes the link advertisement message to the lattice that is published when code invokes the `set_link`
    /// function on a `Host` struct instance. No confirmation or acknowledgement is available for this operation
    /// because it is publish-only.
    pub async fn advertise_link(
        &self,
        actor_id: &str,
        provider_id: &str,
        contract_id: &str,
        link_name: &str,
        values: HashMap<String, String>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::advertise_link(&self.nsprefix);
        trace!("advertise_link:publish {}", &subject);
        let ld = LinkDefinition {
            actor_id: actor_id.to_string(),
            provider_id: provider_id.to_string(),
            contract_id: contract_id.to_string(),
            link_name: link_name.to_string(),
            values,
        };
        let bytes = crate::json_serialize(&ld)?;
        match self
            .nc
            .request_timeout(&subject, &bytes, self.timeout)
            .await
        {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.data)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive advertise link acknowledgement: {}", e).into()),
        }
    }

    /// Publishes a request to remove a link definition to the lattice.
    pub async fn remove_link(
        &self,
        actor_id: &str,
        contract_id: &str,
        link_name: &str,
    ) -> Result<CtlOperationAck> {
        let subject = broker::remove_link(&self.nsprefix);
        let ld = LinkDefinition {
            actor_id: actor_id.to_string(),
            contract_id: contract_id.to_string(),
            link_name: link_name.to_string(),
            ..Default::default()
        };
        let bytes = crate::json_serialize(&ld)?;
        match self
            .nc
            .request_timeout(&subject, &bytes, self.timeout)
            .await
        {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.data)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive remove link acknowledgement: {}", e).into()),
        }
    }

    /// Publishes a request to retrieve all current link definitions.
    pub async fn query_links(&self) -> Result<LinkDefinitionList> {
        let subject = broker::queries::link_definitions(&self.nsprefix);
        match self
            .nc
            .request_timeout(&subject, vec![], self.timeout)
            .await
        {
            Ok(msg) => json_deserialize(&msg.data),
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
    pub async fn update_actor(
        &self,
        host_id: &str,
        existing_actor_id: &str,
        new_actor_ref: &str,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::commands::update_actor(&self.nsprefix, host_id);
        trace!("update_actor:request {}", &subject);
        let bytes = json_serialize(UpdateActorCommand {
            host_id: host_id.to_string(),
            actor_id: existing_actor_id.to_string(),
            new_actor_ref: new_actor_ref.to_string(),
            annotations,
        })?;
        match self
            .nc
            .request_timeout(&subject, &bytes, self.timeout)
            .await
        {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.data)?;
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
    /// appropriate event
    pub async fn start_provider(
        &self,
        host_id: &str,
        provider_ref: &str,
        link_name: Option<String>,
        annotations: Option<HashMap<String, String>>,
        provider_configuration: Option<String>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::commands::start_provider(&self.nsprefix, host_id);
        trace!("start_provider:request {}", &subject);
        let bytes = json_serialize(StartProviderCommand {
            host_id: host_id.to_string(),
            provider_ref: provider_ref.to_string(),
            link_name: link_name.unwrap_or_else(|| "default".to_string()),
            annotations,
            configuration: provider_configuration,
        })?;
        match self
            .nc
            .request_timeout(&subject, &bytes, self.timeout)
            .await
        {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.data)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive start provider acknowledgement: {}", e).into()),
        }
    }

    /// Issues a command to a host to stop a provider for the given OCI reference, link name, and contract ID. The
    /// target wasmCloud host will acknowledge the receipt of this command, and _will not_ supply a discrete
    /// confirmation that a provider has terminated. For that kind of information, the client must also monitor
    /// the control event stream
    pub async fn stop_provider(
        &self,
        host_id: &str,
        provider_ref: &str,
        link_name: &str,
        contract_id: &str,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::commands::stop_provider(&self.nsprefix, host_id);
        trace!("stop_provider:request {}", &subject);
        let bytes = json_serialize(StopProviderCommand {
            host_id: host_id.to_string(),
            provider_ref: provider_ref.to_string(),
            link_name: link_name.to_string(),
            contract_id: contract_id.to_string(),
            annotations,
        })?;
        match self
            .nc
            .request_timeout(&subject, &bytes, self.timeout)
            .await
        {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.data)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive stop provider acknowledgement: {}", e).into()),
        }
    }

    /// Issues a command to a host to stop an actor for the given OCI reference. The
    /// target wasmCloud host will acknowledge the receipt of this command, and _will not_ supply a discrete
    /// confirmation that the actor has terminated. For that kind of information, the client must also monitor
    /// the control event stream
    pub async fn stop_actor(
        &self,
        host_id: &str,
        actor_ref: &str,
        count: u16,
        annotations: Option<HashMap<String, String>>,
    ) -> Result<CtlOperationAck> {
        let subject = broker::commands::stop_actor(&self.nsprefix, host_id);
        trace!("stop_actor:request {}", &subject);
        let bytes = json_serialize(StopActorCommand {
            host_id: host_id.to_string(),
            actor_ref: actor_ref.to_string(),
            count: Some(count),
            annotations,
        })?;
        match self
            .nc
            .request_timeout(&subject, &bytes, self.timeout)
            .await
        {
            Ok(msg) => {
                let ack: CtlOperationAck = json_deserialize(&msg.data)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive stop actor acknowledgement: {}", e).into()),
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
    ///   let nc = nats::asynk::connect("0.0.0.0:4222").await.unwrap();
    ///   let client = Client::new(nc, None, std::time::Duration::from_millis(1000));
    ///   let receiver = client.events_receiver().await.unwrap();
    ///   std::thread::spawn(move || loop {
    ///     if let Ok(evt) = receiver.recv() {
    ///       println!("Event received: {:?}", evt);
    ///     } else {
    ///       // channel is closed
    ///       break;
    ///     }
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
    ///   let nc = nats::asynk::connect("0.0.0.0:4222").await.unwrap();
    ///   let client = Client::new(nc, None, std::time::Duration::from_millis(1000));
    ///   let receiver = client.events_receiver().await.unwrap();
    ///   std::thread::spawn(move || {
    ///     if let Ok(evt) = receiver.recv() {
    ///       println!("Event received: {:?}", evt);
    ///       // We received our one event, now close the channel
    ///       drop(receiver);
    ///     } else {
    ///       // channel is closed
    ///       return;
    ///     }
    ///   });
    /// };
    /// ```
    pub async fn events_receiver(&self) -> Result<Receiver<Event>> {
        let (sender, receiver) = unbounded();
        let sub = self
            .nc
            .subscribe(&broker::control_event(&self.nsprefix))
            .await?;
        std::thread::spawn(move || loop {
            if let Some(msg) = block_on(sub.next()) {
                match json_deserialize::<Event>(&msg.data) {
                    Ok(evt) => {
                        trace!("received event: {:?}", evt);
                        // If the channel is disconnected, stop sending events
                        if sender.send(evt).is_err() {
                            let _ = block_on(sub.unsubscribe());
                            return;
                        }
                    }
                    _ => error!("Object received on event stream was not a CloudEvent"),
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
    serde_json::from_slice(buf).map_err(|e| format!("JSON deserialization failure: {}", e).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Note: This test is a means of manually watching the event stream as CloudEvents are received
    /// It does not assert functionality, and so we've marked it as ignore to ensure it's not run by default
    #[tokio::test]
    #[ignore]
    async fn test_events_receiver() {
        let nc = nats::asynk::connect("0.0.0.0:4222").await.unwrap();
        let client = Client::new(nc, None, std::time::Duration::from_millis(1000));
        let receiver = client.events_receiver().await.unwrap();
        std::thread::spawn(move || loop {
            if let Ok(evt) = receiver.recv() {
                println!("Event received: {:?}", evt);
            } else {
                println!("Channel closed");
                break;
            }
        });
        std::thread::park();
    }
}
