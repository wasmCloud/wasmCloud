use anyhow::Result;
use clap::Parser;

use crate::{
    actor::{scale_component, ScaleComponentArgs},
    cli::{input_vec_to_hashmap, CliConnectionOpts, CommandOutput},
    common::find_host_id,
    config::WashConnectionOptions,
};

use super::validate_component_id;

#[derive(Debug, Clone, Parser)]
pub enum ScaleCommand {
    /// Scale a component running in a host to a certain level of concurrency
    #[clap(name = "component", alias = "actor")]
    Component(ScaleComponentCommand),
}

#[derive(Debug, Clone, Parser)]
pub struct ScaleComponentCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// ID of host to scale component on. If a non-ID is provided, the host will be selected based on
    /// matching the friendly name and will return an error if more than one host matches.
    #[clap(name = "host-id")]
    pub host_id: String,

    /// Component reference, e.g. the absolute file path or OCI URL.
    #[clap(name = "component-ref")]
    pub component_ref: String,

    /// Unique ID to use for the component
    #[clap(name = "component-id", value_parser = validate_component_id)]
    pub component_id: String,

    /// Maximum number of component instances allowed to run concurrently. Setting this value to `0` will stop the component.
    #[clap(short = 'c', long = "max-instances", alias = "max-concurrent", alias = "max", alias = "count", default_value_t = u32::MAX)]
    pub max_instances: u32,

    /// Optional set of annotations used to describe the nature of this component scale command.
    /// For example, autonomous agents may wish to “tag” scale requests as part of a given deployment
    #[clap(short = 'a', long = "annotations")]
    pub annotations: Vec<String>,
}

pub async fn handle_scale_component(cmd: ScaleComponentCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let annotations = input_vec_to_hashmap(cmd.annotations)?;

    scale_component(ScaleComponentArgs {
        client: &client,
        // NOTE(thomastaylor312): In the future, we could check if this is interactive and then
        // prompt the user to choose if more than one thing matches
        host_id: &find_host_id(&cmd.host_id, &client).await?.0,
        component_id: &cmd.component_id,
        component_ref: &cmd.component_ref,
        max_instances: cmd.max_instances,
        annotations: Some(annotations),
        config: vec![],
        skip_wait: false,
        timeout_ms: None,
    })
    .await?;

    let scale_msg = if cmd.max_instances == u32::MAX {
        "unbounded concurrency".to_string()
    } else {
        format!("{} max concurrent instances", cmd.max_instances)
    };

    Ok(CommandOutput::from_key_and_text(
        "result",
        format!(
            "Request to scale actor {} to {scale_msg} has been accepted",
            cmd.component_ref
        ),
    ))
}
