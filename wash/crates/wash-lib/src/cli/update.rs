use anyhow::{bail, Result};
use clap::Parser;

use crate::{
    actor::update_actor,
    config::WashConnectionOptions,
    id::{ModuleId, ServerId},
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

    /// Id of host
    #[clap(name = "host-id", value_parser)]
    pub host_id: ServerId,

    /// Actor Id, e.g. the public key for the actor
    #[clap(name = "actor-id", value_parser)]
    pub actor_id: ModuleId,

    /// Actor reference, e.g. the OCI URL for the actor.
    #[clap(name = "new-actor-ref")]
    pub new_actor_ref: String,
}

pub async fn handle_update_actor(cmd: UpdateActorCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let ack = update_actor(&client, &cmd.host_id, &cmd.actor_id, &cmd.new_actor_ref).await?;
    if !ack.accepted {
        bail!("Operation failed: {}", ack.error);
    }

    Ok(CommandOutput::from_key_and_text(
        "result",
        format!("Actor {} updated to {}", cmd.actor_id, cmd.new_actor_ref),
    ))
}
