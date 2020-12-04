pub mod broker;
mod generated;

pub use crate::generated::ctliface::*;
use actix_rt::time::delay_for;
use futures::stream::StreamExt;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

type Result<T> = ::std::result::Result<T, Box<dyn ::std::error::Error + Send + Sync>>;

pub struct Client {
    nc: nats::asynk::Connection,
    nsprefix: Option<String>,
    timeout: Duration,
}

impl Client {
    pub fn new(nc: nats::asynk::Connection, nsprefix: Option<String>, timeout: Duration) -> Self {
        Client {
            nc,
            nsprefix,
            timeout,
        }
    }

    pub async fn get_hosts(&self, timeout: Duration) -> Result<Vec<Host>> {
        let subject = broker::queries::hosts(&self.nsprefix);

        self.nc
            .request_multi(&subject, vec![])
            .await?
            .map(|m| deserialize::<Host>(&m.data))
            .take_until(delay_for(timeout))
            .try_collect()
            .await
    }

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
        self.nc
            .request_multi(&subject, bytes)
            .await?
            .map(|m| deserialize::<ActorAuctionAck>(&m.data))
            .take_until(delay_for(timeout))
            .try_collect()
            .await
    }

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
        self.nc
            .request_multi(&subject, bytes)
            .await?
            .map(|m| deserialize::<ProviderAuctionAck>(&m.data))
            .take_until(delay_for(timeout))
            .try_collect()
            .await
    }

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
            link_name: link_name.unwrap_or("default".to_string()),
        })?;
        match actix_rt::time::timeout(self.timeout, self.nc.request(&subject, &bytes)).await? {
            Ok(msg) => {
                let ack: StartProviderAck = deserialize(&msg.data)?;
                Ok(ack)
            }
            Err(e) => Err(format!("Did not receive start provider acknowledgement: {}", e).into()),
        }
    }

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
    serde_json::to_vec(&item).map_err(|e| "JSON serialization failure".into())
}

/// The standard function for de-serializing codec structs from a format suitable
/// for message exchange between actor and host. Use of any other function to
/// deserialize could result in breaking incompatibilities.
pub fn deserialize<'de, T: Deserialize<'de>>(
    buf: &'de [u8],
) -> ::std::result::Result<T, Box<dyn std::error::Error + Send + Sync>> {
    serde_json::from_slice(buf).map_err(|e| "JSON deserialization failure".into())
}
