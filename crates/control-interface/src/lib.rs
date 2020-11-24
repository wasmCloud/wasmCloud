pub mod broker;
mod generated;

pub use crate::generated::ctliface::*;
use actix_rt::time::delay_for;
use futures::stream::StreamExt;
use futures::TryStreamExt;
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

    pub async fn get_hosts(&self) -> Result<Vec<Host>> {
        let subject = broker::queries::hosts(&self.nsprefix);

        self.nc
            .request_multi(&subject, vec![])
            .await?
            .map(|m| deserialize::<Host>(&m.data))
            .take_until(delay_for(self.timeout))
            .try_collect()
            .await
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
