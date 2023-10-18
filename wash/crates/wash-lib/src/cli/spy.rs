use anyhow::Result;
use clap::Parser;
use futures::StreamExt;

use super::{CliConnectionOpts, CommandOutput};
use crate::{config::WashConnectionOptions, spier::Spier};

#[derive(Debug, Parser, Clone)]
pub struct SpyCommand {
    /// Actor ID or name to match on. If a name is provided, we will attempt to resolve it to an ID
    /// by checking if the actor_name or call_alias fields from the actor's claims contains the
    /// given string. If more than one matches, then an error will be returned indicating the
    /// options to choose from
    #[clap(name = "actor")]
    pub actor: String,

    #[clap(flatten)]
    pub opts: CliConnectionOpts,
}

/// Handles the spy command, printing all output to stdout until the command is interrupted
pub async fn handle_command(cmd: SpyCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let ctl_client = wco.clone().into_ctl_client(None).await?;
    let nats_client = wco.into_nats_client().await?;

    let mut spier = Spier::new(&cmd.actor, &ctl_client, &nats_client).await?;

    println!("Spying on actor {}\n", spier.actor_id());

    while let Some(msg) = spier.next().await {
        println!(
            r#"
[{}]
From: {:<25}To: {:<25}Host: {}

Operation: {}
Message: {}"#,
            msg.timestamp,
            msg.from,
            msg.to,
            msg.invocation.host_id,
            msg.invocation.operation,
            msg.message
        );
    }

    println!("Message subscribers closed");

    Ok(CommandOutput::default())
}
