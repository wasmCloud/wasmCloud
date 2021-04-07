pub mod broker;
mod generated;
mod inv;
mod sub_stream;

pub use crate::generated::ctliface::*;
use inv::Entity;
pub use inv::{Invocation, InvocationResponse};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};
use sub_stream::SubscriptionStream;
use wascap::prelude::KeyPair;

type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error + Send + Sync>>;

pub struct Client {
    nc: nats::asynk::Connection,
    nsprefix: Option<String>,
    timeout: Duration,
    key: KeyPair,
}

impl Client {
    /// Creates a new lattice control interface client
    pub fn new(nc: nats::asynk::Connection, nsprefix: Option<String>, timeout: Duration) -> Self {
        Client {
            nc,
            nsprefix,
            timeout,
            key: KeyPair::new_server(),
        }
    }

    /// Queries the lattice for all responsive hosts, waiting for the full period specified by _timeout_.
    pub async fn get_hosts(&self, timeout: Duration) -> Result<Vec<Host>> {
        let subject = broker::queries::hosts(&self.nsprefix);
        let sub = self.nc.request_multi(&subject, vec![]).await?;
        let hosts = SubscriptionStream::new(sub)
            .collect(timeout, "get hosts")
            .await;
        Ok(hosts)
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
        let bytes = serialize(ActorAuctionRequest {
            actor_ref: actor_ref.to_string(),
            constraints,
        })?;
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
        let bytes = serialize(ProviderAuctionRequest {
            provider_ref: provider_ref.to_string(),
            link_name: link_name.to_string(),
            constraints,
        })?;
        let sub = self.nc.request_multi(&subject, bytes).await?;
        let providers = SubscriptionStream::new(sub)
            .collect(timeout, "provider auction")
            .await;
        Ok(providers)
    }

    /// Retrieves the contents of a running host
    pub async fn get_host_inventory(&self, host_id: &str) -> Result<HostInventory> {
        let subject = broker::queries::host_inventory(&self.nsprefix, host_id);
        match actix_rt::time::timeout(self.timeout, self.nc.request(&subject, vec![])).await? {
            Ok(msg) => {
                let hi: HostInventory = deserialize(&msg.data)?;
                Ok(hi)
            }
            Err(e) => Err(format!("Did not receive host inventory from target host: {}", e).into()),
        }
    }

    /// Sends a request to the given host to start a given actor by its OCI reference. This returns an acknowledgement
    /// of _receipt_ of the command, not a confirmation that the actor started. An acknowledgement will either indicate
    /// some form of validation failure, or, if no failure occurs, the receipt of the command. To avoid blocking consumers,
    /// wasmCloud hosts will acknowledge the start actor command prior to fetching the actor's OCI bytes. If a client needs
    /// deterministic results as to whether the actor completed its startup process, the client will have to monitor
    /// the appropriate event in the control event stream
    pub async fn start_actor(&self, host_id: &str, actor_ref: &str) -> Result<StartActorAck> {
        let subject = broker::commands::start_actor(&self.nsprefix, host_id);
        let bytes = serialize(StartActorCommand {
            actor_ref: actor_ref.to_string(),
            host_id: host_id.to_string(),
        })?;
        match actix_rt::time::timeout(self.timeout, self.nc.request(&subject, &bytes)).await? {
            Ok(msg) => {
                let ack: StartActorAck = deserialize(&msg.data)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive start actor acknowledgement: {}", e).into()),
        }
    }

    /// Performs a remote procedure call over the lattice, targeting the given actor. This call will appear
    /// to originate from the "system" actor and from a unique host ID that was generated by the control
    /// interface client when it was instantiated. If there are multiple actors with the same public key
    /// actively running in the lattice, then the message broker is responsible for choosing the appropriate
    /// target. Under current NATS implementations, that means an actor is chosen psuedo-randomly among the
    /// known queue subscribers, and will **not** be invoked in round-robin fashion
    pub async fn call_actor(
        &self,
        target_id: &str,
        operation: &str,
        data: &[u8],
    ) -> Result<InvocationResponse> {
        let subject = broker::rpc::call_actor(&self.nsprefix, target_id);
        let bytes = crate::generated::ctliface::serialize(Invocation::new(
            &self.key,
            Entity::Actor("system".to_string()),
            Entity::Actor(target_id.to_string()),
            operation,
            data.to_vec(),
        ))?;
        match actix_rt::time::timeout(self.timeout, self.nc.request(&subject, &bytes)).await? {
            Ok(msg) => {
                let resp: InvocationResponse = crate::generated::ctliface::deserialize(&msg.data)?;
                Ok(resp)
            }
            Err(e) => Err(format!("Actor RPC call did not succeed: {}", e).into()),
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
    ) -> Result<()> {
        let subject = broker::rpc::advertise_links(&self.nsprefix);
        let ld = LinkDefinition {
            actor_id: actor_id.to_string(),
            provider_id: provider_id.to_string(),
            contract_id: contract_id.to_string(),
            link_name: link_name.to_string(),
            values,
        };
        let bytes = crate::generated::ctliface::serialize(&ld)?;
        self.nc.publish(&subject, &bytes).await?;

        Ok(())
    }

    /// Publishes a request to remove a link definition to the lattice. All hosts in the lattice will
    /// receive this message and, if the appropriate capability provider is in that host, it will have
    /// the "remove actor" operation sent to it. The link definition will also be removed from the lattice
    /// cache. No confirmation or acknowledgement is available for this operation, you will need to monitor events
    /// and/or query the lattice to confirm that the link has been removed.
    pub async fn remove_link(
        &self,
        actor_id: &str,
        contract_id: &str,
        link_name: &str,
    ) -> Result<()> {
        let subject = broker::rpc::remove_links(&self.nsprefix);
        let ld = LinkDefinition {
            actor_id: actor_id.to_string(),
            contract_id: contract_id.to_string(),
            link_name: link_name.to_string(),
            ..Default::default()
        };
        let bytes = crate::generated::ctliface::serialize(&ld)?;
        self.nc.publish(&subject, &bytes).await?;

        Ok(())
    }

    /// Issue a command to a host instructing that it replace an existing actor (indicated by its
    /// public key) with a new actor indicated by an OCI image reference. The host will acknowledge
    /// this request as soon as it verifies that the target actor is running. This acknowledgement
    /// occurs **before** the new bytes are downloaded. Live-updating an actor can take a long
    /// time and control clients cannot block waiting for a reply that could come several seconds
    /// later. If you need to verify that the actor has been updated, you will want to set up a
    /// listener for the appropriate **ControlEvent** which will be published on the control events
    /// channel in JSON
    pub async fn update_actor(
        &self,
        host_id: &str,
        existing_actor_id: &str,
        new_actor_ref: &str,
    ) -> Result<UpdateActorAck> {
        let subject = broker::commands::update_actor(&self.nsprefix, host_id);
        let bytes = serialize(UpdateActorCommand {
            host_id: host_id.to_string(),
            actor_id: existing_actor_id.to_string(),
            new_actor_ref: new_actor_ref.to_string(),
        })?;
        match actix_rt::time::timeout(self.timeout, self.nc.request(&subject, &bytes)).await? {
            Ok(msg) => {
                let ack: UpdateActorAck = deserialize(&msg.data)?;
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
    ) -> Result<StartProviderAck> {
        let subject = broker::commands::start_provider(&self.nsprefix, host_id);
        let bytes = serialize(StartProviderCommand {
            host_id: host_id.to_string(),
            provider_ref: provider_ref.to_string(),
            link_name: link_name.unwrap_or_else(|| "default".to_string()),
        })?;
        match actix_rt::time::timeout(self.timeout, self.nc.request(&subject, &bytes)).await? {
            Ok(msg) => {
                let ack: StartProviderAck = deserialize(&msg.data)?;
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
    ) -> Result<StopProviderAck> {
        let subject = broker::commands::stop_provider(&self.nsprefix, host_id);
        let bytes = serialize(StopProviderCommand {
            host_id: host_id.to_string(),
            provider_ref: provider_ref.to_string(),
            link_name: link_name.to_string(),
            contract_id: contract_id.to_string(),
        })?;
        match actix_rt::time::timeout(self.timeout, self.nc.request(&subject, &bytes)).await? {
            Ok(msg) => {
                let ack: StopProviderAck = deserialize(&msg.data)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive stop provider acknowledgement: {}", e).into()),
        }
    }

    /// Issues a command to a host to stop an actor for the given OCI reference. The
    /// target wasmCloud host will acknowledge the receipt of this command, and _will not_ supply a discrete
    /// confirmation that the actor has terminated. For that kind of information, the client must also monitor
    /// the control event stream
    pub async fn stop_actor(&self, host_id: &str, actor_ref: &str) -> Result<StopActorAck> {
        let subject = broker::commands::stop_actor(&self.nsprefix, host_id);
        let bytes = serialize(StopActorCommand {
            host_id: host_id.to_string(),
            actor_ref: actor_ref.to_string(),
        })?;
        match actix_rt::time::timeout(self.timeout, self.nc.request(&subject, &bytes)).await? {
            Ok(msg) => {
                let ack: StopActorAck = deserialize(&msg.data)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive stop actor acknowledgement: {}", e).into()),
        }
    }

    /// Retrieves the full set of all cached claims in the lattice by getting a response from the first
    /// host that answers this query
    pub async fn get_claims(&self) -> Result<ClaimsList> {
        let subject = broker::queries::claims(&self.nsprefix);
        match actix_rt::time::timeout(self.timeout, self.nc.request(&subject, vec![])).await? {
            Ok(msg) => {
                let list: ClaimsList = deserialize(&msg.data)?;
                Ok(list)
            }
            Err(e) => Err(format!("Did not receive claims from lattice: {}", e).into()),
        }
    }
}

/// The standard function for serializing codec structs into a format that can be
/// used for message exchange between actor and host. Use of any other function to
/// serialize could result in breaking incompatibilities.
pub fn serialize<T>(
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
pub fn deserialize<'de, T: Deserialize<'de>>(
    buf: &'de [u8],
) -> ::std::result::Result<T, Box<dyn std::error::Error + Send + Sync>> {
    serde_json::from_slice(buf).map_err(|e| format!("JSON deserialization failure: {}", e).into())
}
