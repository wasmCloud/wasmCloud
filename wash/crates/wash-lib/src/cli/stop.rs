use anyhow::{bail, Result};
use clap::Parser;
use std::collections::HashMap;
use tokio::time::Duration;

use crate::{
    actor::stop_actor,
    cli::{CliConnectionOpts, CommandOutput},
    common::boxed_err_to_anyhow,
    config::WashConnectionOptions,
    context::default_timeout_ms,
    id::{validate_contract_id, ModuleId, ServerId, ServiceId},
    wait::{wait_for_provider_stop_event, ActorStoppedInfo, FindEventOutcome, ProviderStoppedInfo},
};

#[derive(Debug, Clone, Parser)]
pub enum StopCommand {
    /// Stop an actor running in a host
    #[clap(name = "actor")]
    Actor(StopActorCommand),

    /// Stop a provider running in a host
    #[clap(name = "provider")]
    Provider(StopProviderCommand),

    /// Purge and stop a running host
    #[clap(name = "host")]
    Host(StopHostCommand),
}

#[derive(Debug, Clone, Parser)]
pub struct StopActorCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Id of host
    #[clap(name = "host-id", value_parser)]
    pub host_id: ServerId,

    /// Actor Id, e.g. the public key for the actor
    #[clap(name = "actor-id", value_parser)]
    pub actor_id: ModuleId,

    /// Number of actors to stop
    #[clap(long = "count", default_value = "1")]
    pub count: u16,

    /// By default, the command will wait until the actor has been stopped.
    /// If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the actor to stp[].
    #[clap(long = "skip-wait")]
    pub skip_wait: bool,
}

#[derive(Debug, Clone, Parser)]
pub struct StopProviderCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Id of host
    #[clap(name = "host-id", value_parser)]
    pub host_id: ServerId,

    /// Provider Id, e.g. the public key for the provider
    #[clap(name = "provider-id", value_parser)]
    pub provider_id: ServiceId,

    /// Link name of provider
    #[clap(name = "link-name")]
    pub link_name: String,

    /// Capability contract Id of provider
    #[clap(name = "contract-id")]
    pub contract_id: String,

    /// By default, the command will wait until the provider has been stopped.
    /// If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the provider to stop.
    #[clap(long = "skip-wait")]
    pub skip_wait: bool,
}

#[derive(Debug, Clone, Parser)]
pub struct StopHostCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Id of host
    #[clap(name = "host-id", value_parser)]
    pub host_id: ServerId,

    /// The timeout in ms for how much time to give the host for graceful shutdown
    #[clap(
        long = "host-timeout",
        default_value_t = default_timeout_ms()
    )]
    pub host_shutdown_timeout: u64,
}

pub async fn stop_provider(cmd: StopProviderCommand) -> Result<CommandOutput> {
    validate_contract_id(&cmd.contract_id)?;
    let timeout_ms = cmd.opts.timeout_ms;
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let mut receiver = client
        .events_receiver()
        .await
        .map_err(boxed_err_to_anyhow)?;

    let ack = client
        .stop_provider(
            &cmd.host_id,
            &cmd.provider_id,
            &cmd.link_name,
            &cmd.contract_id,
            None,
        )
        .await
        .map_err(boxed_err_to_anyhow)?;

    if !ack.accepted {
        bail!("Operation failed: {}", ack.error);
    }
    if cmd.skip_wait {
        let text = format!("Provider {} stop request received", cmd.provider_id);
        return Ok(CommandOutput::new(
            text.clone(),
            HashMap::from([
                ("result".into(), text.into()),
                ("provider_id".into(), cmd.provider_id.to_string().into()),
                ("link_name".into(), cmd.link_name.into()),
                ("contract_id".into(), cmd.contract_id.into()),
                ("host_id".into(), cmd.host_id.to_string().into()),
            ]),
        ));
    }

    let event = wait_for_provider_stop_event(
        &mut receiver,
        Duration::from_millis(timeout_ms),
        cmd.host_id.to_string(),
        cmd.provider_id.to_string(),
    )
    .await?;

    match event {
        FindEventOutcome::Success(ProviderStoppedInfo {
            host_id,
            provider_id,
            link_name,
            contract_id,
        }) => {
            let text = format!("Provider [{}] stopped successfully", &provider_id);
            Ok(CommandOutput::new(
                text.clone(),
                HashMap::from([
                    ("result".into(), text.into()),
                    ("provider_id".into(), provider_id.into()),
                    ("host_id".into(), host_id.into()),
                    ("link_name".into(), link_name.into()),
                    ("contract_id".into(), contract_id.into()),
                ]),
            ))
        }
        FindEventOutcome::Failure(err) => bail!("{}", err),
    }
}

pub async fn handle_stop_actor(cmd: StopActorCommand) -> Result<CommandOutput> {
    let timeout_ms = cmd.opts.timeout_ms;
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let ActorStoppedInfo { actor_id, host_id } = stop_actor(
        &client,
        &cmd.host_id,
        &cmd.actor_id,
        None,
        timeout_ms,
        cmd.skip_wait,
    )
    .await?;

    let text = if cmd.skip_wait {
        format!("Request to stop actor {} received", &actor_id)
    } else {
        format!("Actor [{}] stopped", &actor_id)
    };

    Ok(CommandOutput::new(
        text.clone(),
        HashMap::from([
            ("result".into(), text.into()),
            ("actor_id".into(), actor_id.into()),
            ("host_id".into(), host_id.into()),
        ]),
    ))
}

pub async fn stop_host(cmd: StopHostCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;
    let ack = client
        .stop_host(&cmd.host_id, Some(cmd.host_shutdown_timeout))
        .await
        .map_err(boxed_err_to_anyhow)?;

    if !ack.accepted {
        bail!("Operation failed: {}", ack.error);
    }

    Ok(CommandOutput::from_key_and_text(
        "result",
        format!("Host {} acknowledged stop request", cmd.host_id),
    ))
}
