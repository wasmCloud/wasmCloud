use std::collections::HashMap;

use anyhow::Result;
use clap::Parser;

use crate::lib::cli::{input_vec_to_hashmap, CliConnectionOpts, CommandOutput};
use crate::lib::common::find_host_id;
use crate::lib::component::{scale_component, ScaleComponentArgs};
use crate::lib::config::WashConnectionOptions;
use crate::lib::context::default_component_operation_timeout_ms;

use super::start::resolve_ref;
use super::validate_component_id;

#[derive(Debug, Clone, Parser)]
pub enum ScaleCommand {
    /// Scale a component running in a host to a certain level of concurrency
    #[clap(name = "component")]
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

    /// List of named configuration to apply to the component, may be empty
    #[clap(long = "config")]
    pub config: Vec<String>,

    /// By default, the command will wait until the component has been scaled.
    /// If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the component to be scaled.
    /// If this flag is omitted, the command will wait until the scaled event has been acknowledged.
    #[clap(long = "skip-wait")]
    pub skip_wait: bool,

    /// Timeout for waiting for scale to occur (normally on an auction response), defaults to 2000 milliseconds
    #[clap(long = "wait-timeout-ms", default_value_t = default_component_operation_timeout_ms())]
    pub wait_timeout_ms: u64,
}

pub async fn handle_scale_component(cmd: ScaleComponentCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let annotations = input_vec_to_hashmap(cmd.annotations)?;
    let component_ref = resolve_ref(&cmd.component_ref).await?;

    let info = scale_component(ScaleComponentArgs {
        client: &client,
        // NOTE(thomastaylor312): In the future, we could check if this is interactive and then
        // prompt the user to choose if more than one thing matches
        host_id: &find_host_id(&cmd.host_id, &client).await?.0,
        component_id: &cmd.component_id,
        component_ref: &component_ref,
        max_instances: cmd.max_instances,
        annotations: Some(annotations),
        config: cmd.config,
        skip_wait: cmd.skip_wait,
        timeout_ms: None,
    })
    .await?;

    let scale_msg = if cmd.max_instances == u32::MAX {
        "unbounded concurrency".to_string()
    } else {
        format!("{} max concurrent instances", cmd.max_instances)
    };

    let text = format!(
        "Component [{}] (ref: [{}]) scaled on host [{}] to {scale_msg}",
        info.component_id, info.component_ref, info.host_id,
    );
    Ok(CommandOutput::new(
        text.clone(),
        HashMap::from([
            ("host_id".into(), info.host_id.into()),
            ("component_id".into(), info.component_id.into()),
            ("component_ref".into(), info.component_ref.into()),
            ("result".into(), text.into()),
        ]),
    ))
}
