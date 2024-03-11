use anyhow::{Context, Result};
use clap::Parser;
use wasmcloud_control_interface::{Host, HostInventory};

use crate::{
    common::{boxed_err_to_anyhow, get_all_inventories},
    config::WashConnectionOptions,
    id::ServerId,
};

use super::CliConnectionOpts;

#[derive(Debug, Clone, Parser)]
pub struct GetClaimsCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,
}

#[derive(Debug, Clone, Parser)]
pub struct GetHostInventoriesCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Host ID to retrieve inventory for. If not provided, wash will query the inventories of all running hosts.
    #[clap(name = "host-id", value_parser)]
    pub host_id: Option<ServerId>,
}

#[derive(Debug, Clone, Parser)]
pub struct GetLinksCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,
}

#[derive(Debug, Clone, Parser)]
pub struct GetHostsCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,
}

#[derive(Debug, Clone, Parser)]
pub enum GetCommand {
    /// Retrieve all known links in the lattice
    #[clap(name = "links")]
    Links(GetLinksCommand),

    /// Retrieve all known claims inside the lattice
    #[clap(name = "claims")]
    Claims(GetClaimsCommand),

    /// Retrieve all responsive hosts in the lattice
    #[clap(name = "hosts")]
    Hosts(GetHostsCommand),

    /// Retrieve inventory a given host on in the lattice
    #[clap(name = "inventory", alias = "inventories")]
    HostInventories(GetHostInventoriesCommand),
}

/// Retrieve host inventory
pub async fn get_host_inventories(cmd: GetHostInventoriesCommand) -> Result<Vec<HostInventory>> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    if let Some(host_id) = cmd.host_id {
        if let Some(inventory) = client
            .get_host_inventory(&host_id)
            .await
            .map(|inventory| inventory.response)
            .map_err(boxed_err_to_anyhow)?
        {
            Ok(vec![inventory])
        } else {
            Ok(vec![])
        }
    } else {
        let hosts = get_all_inventories(&client)
            .await
            .context("unable to fetch all inventory")?;
        match hosts.len() {
            0 => Err(anyhow::anyhow!(
                "No hosts are available for inventory query."
            )),
            _ => Ok(hosts),
        }
    }
}

/// Retrieve hosts
pub async fn get_hosts(cmd: GetHostsCommand) -> Result<Vec<Host>> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;
    client
        .get_hosts()
        .await
        .map_err(boxed_err_to_anyhow)
        .map(|hosts| {
            hosts
                .into_iter()
                .filter_map(|h| h.response)
                .collect::<Vec<_>>()
        })
        .context("Was able to connect to NATS, but failed to get hosts.")
}
