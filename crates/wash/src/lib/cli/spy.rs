use anyhow::Result;
use clap::Parser;
use futures::StreamExt;

use super::{validate_component_id, CliConnectionOpts, CommandOutput};
use crate::lib::{config::WashConnectionOptions, spier::Spier};

#[derive(Debug, Parser, Clone)]
pub struct SpyCommand {
    /// Component ID to spy on, component or capability provider. This is the unique identifier supplied
    /// to the component at startup.
    #[clap(name = "component_id", value_parser = validate_component_id)]
    pub component_id: String,

    #[clap(flatten)]
    pub opts: CliConnectionOpts,
}

/// Handles the spy command, printing all output to stdout until the command is interrupted
pub async fn handle_command(cmd: SpyCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let ctl_client = wco.clone().into_ctl_client(None).await?;
    let nats_client = wco.into_nats_client().await?;

    let mut spier = Spier::new(&cmd.component_id, &ctl_client, &nats_client).await?;

    println!("Spying on component {}\n", spier.component_id());

    while let Some(msg) = spier.next().await {
        println!(
            r"
[{}]
From: {:<25} To: {:<25}

Operation: {}
Message: {}",
            msg.timestamp, msg.from, msg.to, msg.operation, msg.message
        );
    }

    println!("Message subscribers closed");

    Ok(CommandOutput::default())
}
