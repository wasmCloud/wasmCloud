use std::str::FromStr;

use crate::lib::{
    common::{boxed_err_to_anyhow, get_all_inventories},
    config::WashConnectionOptions,
    id::ServerId,
};
use anyhow::{Context, Result};
use clap::Parser;
use wasmcloud_control_interface::{Host, HostInventory};

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

    /// Enables Real-time updates, duration can be specified in ms or in humantime (eg: 2s, 5m, 54ms). Defaults to 5000 milliseconds.
    #[clap(long, short, num_args = 0..=1, default_missing_value = "5000", value_parser = parse_watch_interval)]
    pub watch: Option<std::time::Duration>,
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
            .map(wasmcloud_control_interface::CtlResponse::into_data)
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
                .filter_map(wasmcloud_control_interface::CtlResponse::into_data)
                .collect::<Vec<_>>()
        })
        .context("Was able to connect to NATS, but failed to get hosts.")
}

pub fn parse_watch_interval(arg: &str) -> Result<std::time::Duration, String> {
    if let Ok(duration) = humantime::Duration::from_str(arg) {
        return Ok(duration.into());
    }

    if let Ok(millis) = arg.parse::<u64>() {
        return Ok(std::time::Duration::from_millis(millis));
    }

    Err(format!("Invalid duration: '{arg}'. Expected a duration like '5s', '1m', '100ms', or milliseconds as an integer."))
}
