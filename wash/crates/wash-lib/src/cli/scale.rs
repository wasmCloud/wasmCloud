use anyhow::Result;
use clap::Parser;

use crate::{
    actor::scale_actor,
    cli::{labels_vec_to_hashmap, CliConnectionOpts, CommandOutput},
    config::WashConnectionOptions,
    id::ServerId,
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

    /// Id of host
    #[clap(name = "host-id", value_parser)]
    pub host_id: ServerId,

    /// Actor reference, e.g. the OCI URL for the actor.
    #[clap(name = "actor-ref")]
    pub actor_ref: String,

    /// Maximum number of instances this actor can run concurrently. Omitting this value means there is no maximum.
    #[clap(short = 'c', long = "max-concurrent", alias = "max", alias = "count")]
    pub max_concurrent: Option<u16>,

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
        &cmd.host_id,
        &cmd.actor_ref,
        cmd.max_concurrent,
        Some(annotations),
    )
    .await?;

    let max = cmd
        .max_concurrent
        .map(|max| max.to_string())
        .unwrap_or_else(|| "unbounded".to_string());

    Ok(CommandOutput::from_key_and_text(
        "result",
        format!(
            "Request to scale actor {} to {} max concurrent instances received",
            cmd.actor_ref, max
        ),
    ))
}
