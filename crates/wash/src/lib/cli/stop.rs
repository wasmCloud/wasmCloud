use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use std::collections::HashMap;
use tokio::time::Duration;
use tracing::error;
use wasmcloud_control_interface::HostInventory;

use crate::lib::{
    cli::{CliConnectionOpts, CommandOutput},
    common::{boxed_err_to_anyhow, find_host_id, get_all_inventories, FindIdError, Match},
    component::{scale_component, ComponentScaledInfo, ScaleComponentArgs},
    config::{host_pid_file, WashConnectionOptions},
    context::default_timeout_ms,
    id::ServerId,
    wait::{wait_for_provider_stop_event, FindEventOutcome, ProviderStoppedInfo},
};

use super::validate_component_id;

#[derive(Debug, Clone, Parser)]
pub enum StopCommand {
    /// Stop a component running in a host
    #[clap(name = "component")]
    Component(StopComponentCommand),

    /// Stop a provider running in a host
    #[clap(name = "provider")]
    Provider(StopProviderCommand),

    /// Purge and stop a running host
    #[clap(name = "host")]
    Host(StopHostCommand),
}

#[derive(Debug, Clone, Parser)]
pub struct StopComponentCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Id of host to stop component on. If a non-ID is provided, the host will be selected based
    /// on matching the prefix of the ID or the friendly name and will return an error if more than
    /// one host matches. If no host ID is passed, a host will be selected based on whether or not
    /// the component is running on it. If more than 1 host is running this component, an error will be
    /// returned with a list of hosts running the component
    #[clap(long = "host-id")]
    pub host_id: Option<String>,

    /// Unique component Id or a string to match on the prefix of the ID. If multiple components are matched, then an error
    /// will be returned with a list of all matching options
    #[clap(name = "component-id", value_parser = validate_component_id)]
    pub component_id: String,

    /// By default, the command will wait until the component has been stopped.
    /// If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the component to stp[].
    #[clap(long = "skip-wait")]
    pub skip_wait: bool,
}

#[derive(Debug, Clone, Parser)]
pub struct StopProviderCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Id of host to stop provider on. If a non-ID is provided, the host will be selected based on
    /// matching the prefix of the ID or the friendly name and will return an error if more than one
    /// host matches. If no host ID is passed, a host will be selected based on whether or not the
    /// provider is running on it. If more than 1 host is running this provider, an error will be returned
    /// with a list of hosts running the provider
    #[clap(long = "host-id")]
    pub host_id: Option<String>,

    /// Provider Id (e.g. the public key for the provider) or a string to match on the prefix of the
    /// ID, or friendly name, or call alias of the provider. If multiple providers are matched, then
    /// an error will be returned with a list of all matching options
    #[clap(name = "provider-id", value_parser = validate_component_id)]
    pub provider_id: String,

    /// By default, the command will wait until the provider has been stopped. If this flag is
    /// passed, the command will return immediately after acknowledgement from the host, without
    /// waiting for the provider to stop.
    #[clap(long = "skip-wait")]
    pub skip_wait: bool,
}

#[derive(Debug, Clone, Parser)]
pub struct StopHostCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Id of host to stop. If a non-ID is provided, the host will be selected based on matching the
    /// prefix of the ID or the friendly name and will return an error if more than one host
    /// matches.
    #[clap(name = "host-id")]
    pub host_id: String,

    /// The timeout in ms for how much time to give the host for graceful shutdown
    #[clap(
        long = "host-timeout",
        default_value_t = default_timeout_ms()
    )]
    pub host_shutdown_timeout: u64,
}

pub async fn handle_stop_provider(cmd: StopProviderCommand) -> Result<CommandOutput> {
    let timeout_ms = cmd.opts.timeout_ms;
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let ctl_client = wco.into_ctl_client(None).await?;
    stop_provider(
        &ctl_client,
        cmd.host_id.as_deref(),
        &cmd.provider_id,
        cmd.skip_wait,
        timeout_ms,
    )
    .await?;

    let text = if cmd.skip_wait {
        format!("Provider {} stop request received", &cmd.provider_id)
    } else {
        format!("Provider [{}] stopped successfully", &cmd.provider_id)
    };

    Ok(CommandOutput::new(
        text.clone(),
        HashMap::from([
            ("result".into(), text.into()),
            ("provider_id".into(), cmd.provider_id.into()),
            ("host_id".into(), cmd.host_id.into()),
        ]),
    ))
}

pub async fn stop_provider(
    client: &wasmcloud_control_interface::Client,
    host_id: Option<&str>,
    provider_id: &str,
    skip_wait: bool,
    timeout_ms: u64,
) -> Result<()> {
    let mut receiver = client
        .events_receiver(vec![
            "provider_stopped".to_string(),
            "provider_stop_failed".to_string(),
        ])
        .await
        .map_err(boxed_err_to_anyhow)?;

    let host_id = if let Some(host_id) = host_id {
        find_host_id(host_id, client).await?.0
    } else {
        find_host_with_provider(provider_id, client).await?
    };

    let ack = client
        .stop_provider(&host_id, provider_id)
        .await
        .map_err(boxed_err_to_anyhow)?;

    if !ack.succeeded() {
        bail!("Operation failed: {}", ack.message());
    }
    if skip_wait {
        return Ok(());
    }

    let event = wait_for_provider_stop_event(
        &mut receiver,
        Duration::from_millis(timeout_ms),
        host_id.to_string(),
        provider_id.to_string(),
    )
    .await?;

    match event {
        FindEventOutcome::Success(ProviderStoppedInfo { .. }) => Ok(()),
        FindEventOutcome::Failure(err) => bail!("{}", err),
    }
}

pub async fn handle_stop_component(cmd: StopComponentCommand) -> Result<CommandOutput> {
    let timeout_ms = cmd.opts.timeout_ms;
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let component_id = cmd.component_id;

    let inventory = if let Some(host_id) = cmd.host_id {
        client
            .get_host_inventory(&host_id)
            .await
            .map(wasmcloud_control_interface::CtlResponse::into_data)
            .map_err(boxed_err_to_anyhow)?
            .context("Supplied host did not respond to inventory query")?
    } else {
        let inventories = get_all_inventories(&client).await?;
        inventories
            .into_iter()
            .find(|inv| {
                inv.components()
                    .iter()
                    .any(|component| component.id() == component_id)
            })
            .ok_or_else(|| anyhow::anyhow!("No host found running component [{}]", component_id))?
    };

    let Some((host_id, component_ref)) = inventory
        .components()
        .iter()
        .find(|component| component.id() == component_id)
        .map(|component| {
            (
                inventory.host_id().to_string(),
                component.image_ref().to_string(),
            )
        })
    else {
        bail!(
            "No component with id [{component_id}] found on host [{}]",
            inventory.host_id()
        );
    };

    let ComponentScaledInfo {
        component_id,
        host_id,
        ..
    } = scale_component(ScaleComponentArgs {
        client: &client,
        host_id: &host_id,
        component_id: &component_id,
        component_ref: &component_ref,
        max_instances: 0,
        annotations: None,
        config: vec![],
        skip_wait: cmd.skip_wait,
        timeout_ms: Some(timeout_ms),
    })
    .await?;

    let text = if cmd.skip_wait {
        format!("Request to stop component [{component_id}] received",)
    } else {
        format!("Component [{component_id}] stopped")
    };

    Ok(CommandOutput::new(
        text.clone(),
        HashMap::from([
            ("result".into(), text.into()),
            ("component_id".into(), component_id.into()),
            ("host_id".into(), host_id.into()),
        ]),
    ))
}

pub async fn stop_host(cmd: StopHostCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let (_, hosts_remain) = stop_hosts(client, Some(&cmd.host_id), false).await?;
    let pid_file_exists = tokio::fs::try_exists(host_pid_file()?).await?;
    if !hosts_remain && pid_file_exists {
        tokio::fs::remove_file(host_pid_file()?).await?;
    }

    Ok(CommandOutput::from_key_and_text(
        "result",
        format!("Host {} acknowledged stop request", cmd.host_id),
    ))
}

async fn find_host_with_provider(
    provider_id: &str,
    ctl_client: &wasmcloud_control_interface::Client,
) -> Result<ServerId, FindIdError> {
    find_host_with_filter(ctl_client, |inv| {
        inv.providers()
            .iter()
            .any(|prov| prov.id() == provider_id)
            .then_some((inv.host_id().to_string(), inv.friendly_name().to_string()))
            .and_then(|(id, friendly_name)| id.parse().ok().map(|i| (i, friendly_name)))
    })
    .await
}

async fn find_host_with_filter<F>(
    ctl_client: &wasmcloud_control_interface::Client,
    filter: F,
) -> Result<ServerId, FindIdError>
where
    F: FnMut(HostInventory) -> Option<(ServerId, String)>,
{
    let inventories = get_all_inventories(ctl_client).await?;
    let all_matching = inventories
        .into_iter()
        .filter_map(filter)
        .collect::<Vec<(ServerId, String)>>();

    if all_matching.is_empty() {
        Err(FindIdError::NoMatches)
    } else if all_matching.len() > 1 {
        Err(FindIdError::MultipleMatches(
            all_matching
                .into_iter()
                .map(|(id, friendly_name)| Match {
                    id: id.into_string(),
                    friendly_name: Some(friendly_name),
                })
                .collect(),
        ))
    } else {
        // SAFETY: We know there is exactly one match at this point
        Ok(all_matching.into_iter().next().unwrap().0)
    }
}

/// Stop running wasmCloud hosts, returns a vector of host IDs that were stopped and
/// a boolean indicating whether any hosts remain running
pub async fn stop_hosts(
    client: wasmcloud_control_interface::client::Client,
    host_id: Option<&String>,
    all: bool,
) -> Result<(Vec<String>, bool)> {
    let hosts = client
        .get_hosts()
        .await
        .map_err(|e| anyhow!(e))?
        .into_iter()
        .filter_map(wasmcloud_control_interface::CtlResponse::into_data)
        .collect::<Vec<_>>();

    // If a host ID was supplied, stop only that host
    if let Some(host_id) = host_id {
        let host_id_string = host_id.to_string();
        client.stop_host(&host_id_string, None).await.map_err(|e| {
            anyhow!(
                "Could not stop host, ensure a host with that ID is running: {:?}",
                e
            )
        })?;

        Ok((vec![host_id_string], hosts.len() > 1))
    } else if hosts.is_empty() {
        Ok((vec![], false))
    } else if hosts.len() == 1 {
        let host_id = hosts[0].id();
        client
            .stop_host(host_id, None)
            .await
            .map_err(|e| anyhow!(e))?;
        Ok((vec![host_id.to_string()], false))
    } else if all {
        let host_stops = hosts
            .iter()
            .map(|host| async {
                let host_id = host.id();
                match client.stop_host(host_id, None).await {
                    Ok(_) => Some(host_id.to_owned()),
                    Err(e) => {
                        error!("Could not stop host {}: {:?}", host_id, e);
                        None
                    }
                }
            })
            .collect::<Vec<_>>();
        let all_stops = futures::future::join_all(host_stops).await;
        let host_ids = all_stops
            .iter()
            // Remove any host IDs that ran into errors
            .filter_map(std::borrow::ToOwned::to_owned)
            .collect::<Vec<_>>();
        let hosts_remaining = all_stops.len() > host_ids.len();

        Ok((host_ids, hosts_remaining))
    } else {
        let running_hosts = hosts
            .into_iter()
            .map(|h| h.id().to_string())
            .collect::<Vec<_>>();
        bail!(
            "More than one host is running, please specify a host ID or use --all\nRunning hosts: {running_hosts:?}", 
        )
    }
}
