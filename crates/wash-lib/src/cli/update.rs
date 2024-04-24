use anyhow::{bail, Context, Result};
use clap::Parser;

use crate::{
    actor::update_actor,
    common::{boxed_err_to_anyhow, get_all_inventories},
    config::WashConnectionOptions,
};

use super::{validate_component_id, CliConnectionOpts, CommandOutput};

#[derive(Debug, Clone, Parser)]
pub enum UpdateCommand {
    /// Update a component running in a host to a newer version
    #[clap(name = "component", alias = "actor")]
    Component(UpdateComponentCommand),
}

#[derive(Debug, Clone, Parser)]
pub struct UpdateComponentCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// ID of host to update the component on. If a non-ID is provided, the host will be selected based
    /// on matching the prefix of the ID or the friendly name and will return an error if more than
    /// one host matches. If no host ID is passed, a host will be selected based on whether or not
    /// the component is running on it. If more than 1 host is running this component, an error will be
    /// returned with a list of hosts running the component
    #[clap(long = "host-id")]
    pub host_id: Option<String>,

    /// Unique ID of the component to update
    #[clap(name = "component-id", value_parser = validate_component_id)]
    pub component_id: String,

    /// Component reference to replace the current running comonent with, e.g. the absolute file path or OCI URL.
    #[clap(name = "new-component-ref")]
    pub new_component_ref: String,
}

pub async fn handle_update_actor(cmd: UpdateComponentCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let inventory = if let Some(host_id) = cmd.host_id {
        client
            .get_host_inventory(&host_id)
            .await
            .map(|inventory| inventory.response)
            .map_err(boxed_err_to_anyhow)?
            .context(format!(
                "Supplied host [{}] did not respond to inventory query",
                host_id
            ))?
    } else {
        let inventories = get_all_inventories(&client).await?;
        inventories
            .into_iter()
            .find(|inv| {
                inv.components
                    .iter()
                    .any(|component| component.id == cmd.component_id)
            })
            .ok_or_else(|| {
                anyhow::anyhow!("No host found running component [{}]", cmd.component_id)
            })?
    };

    let Some((host_id, component_ref)) = inventory
        .components
        .iter()
        .find(|component| component.id == cmd.component_id)
        .map(|component| (inventory.host_id.clone(), component.image_ref.clone()))
    else {
        bail!(
            "Component {} not found on host [{}]",
            cmd.component_id,
            inventory.host_id,
        );
    };

    if component_ref == cmd.new_component_ref {
        bail!(
            "Component {} already updated to {} on host [{}]",
            cmd.component_id,
            cmd.new_component_ref,
            host_id
        );
    }

    let ack = update_actor(&client, &host_id, &cmd.component_id, &cmd.new_component_ref).await?;
    if !ack.success {
        bail!("Operation failed on host [{}]: {}", host_id, ack.message);
    }

    let message = match ack.message {
        message if message.is_empty() => format!(
            "component {} updating from {} to {}",
            cmd.component_id, component_ref, cmd.new_component_ref
        ),
        message => message,
    };

    Ok(CommandOutput::from_key_and_text(
        "result",
        format!("Host [{}]: {}", host_id, message),
    ))
}
