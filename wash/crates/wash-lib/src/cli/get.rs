use anyhow::{Context, Result};
use clap::Parser;
use wasmcloud_control_interface::{Host, HostInventory};

use crate::{common::boxed_err_to_anyhow, config::WashConnectionOptions, id::ServerId};

use super::CliConnectionOpts;

#[derive(Debug, Clone, Parser)]
pub struct GetClaimsCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,
}

#[derive(Debug, Clone, Parser)]
pub struct GetHostInventoryCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Host ID to retrieve inventory for. If not provided, wash will attempt to query the inventory of a single running host.
    /// If more than one host is found, an error will be returned prompting for a specific ID.
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
    #[clap(name = "inventory")]
    HostInventory(GetHostInventoryCommand),
}

/// Retreive host inventory
pub async fn get_host_inventory(cmd: GetHostInventoryCommand) -> Result<HostInventory> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let host_id = if let Some(host_id) = cmd.host_id {
        host_id.to_string()
    } else {
        let hosts = client.get_hosts().await.map_err(boxed_err_to_anyhow)?;
        match hosts.len() {
            0 => anyhow::bail!("No hosts are available for inventory query."),
            // SAFETY: We know that the length is 1, so we can safely unwrap the first element
            1 => hosts.first().unwrap().id.clone(),
            _ => {
                anyhow::bail!("No host id provided and more than one host is available. Please specify a host id.")
            }
        }
    };

    client
        .get_host_inventory(&host_id)
        .await
        .map_err(boxed_err_to_anyhow)
        .context("Was able to connect to NATS, but failed to get host inventory.")
}

/// Retrieve hosts
pub async fn get_hosts(cmd: GetHostsCommand) -> Result<Vec<Host>> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;
    client
        .get_hosts()
        .await
        .map_err(boxed_err_to_anyhow)
        .context("Was able to connect to NATS, but failed to get hosts.")
}
