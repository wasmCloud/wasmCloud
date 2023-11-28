use anyhow::Result;
use clap::Parser;

use crate::{
    actor::scale_actor,
    cli::{labels_vec_to_hashmap, CliConnectionOpts, CommandOutput},
    common::find_host_id,
    config::WashConnectionOptions,
};

#[derive(Debug, Clone, Parser)]
pub enum ScaleCommand {
    /// Scale an actor running in a host to a certain level of concurrency
    #[clap(name = "actor")]
    Actor(ScaleActorCommand),
}

#[derive(Debug, Clone, Parser)]
pub struct ScaleActorCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// ID of host to scale actor on. If a non-ID is provided, the host will be selected based on
    /// matching the friendly name and will return an error if more than one host matches.
    #[clap(name = "host-id")]
    pub host_id: String,

    /// Actor reference, e.g. the OCI URL for the actor.
    #[clap(name = "actor-ref")]
    pub actor_ref: String,

    /// Maximum number of instances this actor can run concurrently.
    #[clap(short = 'c', long = "max-instances", alias = "max-concurrent", alias = "max", alias = "count", default_value_t = u16::MAX)]
    pub max_instances: u16,

    /// Optional set of annotations used to describe the nature of this actor scale command.
    /// For example, autonomous agents may wish to “tag” scale requests as part of a given deployment
    #[clap(short = 'a', long = "annotations")]
    pub annotations: Vec<String>,
}

pub async fn handle_scale_actor(cmd: ScaleActorCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let annotations = labels_vec_to_hashmap(cmd.annotations)?;

    scale_actor(
        &client,
        // NOTE(thomastaylor312): In the future, we could check if this is interactive and then
        // prompt the user to choose if more than one thing matches
        &find_host_id(&cmd.host_id, &client).await?.0,
        &cmd.actor_ref,
        cmd.max_instances,
        Some(annotations),
    )
    .await?;

    Ok(CommandOutput::from_key_and_text(
        "result",
        format!(
            "Request to scale actor {} to {} max concurrent instances received",
            cmd.actor_ref, cmd.max_instances
        ),
    ))
}
