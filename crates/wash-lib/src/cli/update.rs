use anyhow::{bail, Result};
use clap::Parser;

use crate::{
    actor::update_actor,
    common::{find_actor_id, find_host_id},
    config::WashConnectionOptions,
};

use super::{CliConnectionOpts, CommandOutput};

#[derive(Debug, Clone, Parser)]
pub enum UpdateCommand {
    /// Update an actor running in a host to a newer version
    #[clap(name = "actor")]
    Actor(UpdateActorCommand),
}

#[derive(Debug, Clone, Parser)]
pub struct UpdateActorCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// ID of host to update the actor on. If a non-ID is provided, the host will be selected based
    /// on matching the prefix of the ID or the friendly name and will return an error if more than
    /// one host matches. If no host ID is passed, a host will be selected based on whether or not
    /// the actor is running on it. If more than 1 host is running this actor, an error will be
    /// returned with a list of hosts running the actor
    #[clap(long = "host-id")]
    pub host_id: Option<String>,

    /// Actor Id (e.g. the public key for the actor) or a string to match on the friendly name or
    /// call alias of the actor. If multiple actors are matched, then an error will be returned with
    /// a list of all matching options
    #[clap(name = "actor-id")]
    pub actor_id: String,

    /// Actor reference, e.g. the OCI URL for the actor.
    #[clap(name = "new-actor-ref")]
    pub new_actor_ref: String,
}

pub async fn handle_update_actor(cmd: UpdateActorCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let (actor_id, friendly_name) = find_actor_id(&cmd.actor_id, &client).await?;

    let host_id = if let Some(host_id) = cmd.host_id {
        find_host_id(&host_id, &client).await?.0
    } else {
        super::stop::find_host_with_actor(&actor_id, &client).await?
    };

    let ack = update_actor(&client, &host_id, &actor_id, &cmd.new_actor_ref).await?;
    if !ack.accepted {
        bail!("Operation failed: {}", ack.error);
    }

    Ok(CommandOutput::from_key_and_text(
        "result",
        format!(
            "Actor {} updated to {}",
            friendly_name.as_deref().unwrap_or(actor_id.as_ref()),
            cmd.new_actor_ref
        ),
    ))
}
