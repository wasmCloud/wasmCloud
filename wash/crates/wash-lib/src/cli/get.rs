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

    /// Id of host
    #[clap(name = "host-id", value_parser)]
    pub host_id: ServerId,
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
    client
        .get_host_inventory(&cmd.host_id)
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
