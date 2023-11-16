use anyhow::{bail, Result};
use clap::Parser;
use std::collections::HashMap;
use tokio::time::Duration;
use wasmcloud_control_interface::HostInventory;

use crate::{
    actor::stop_actor,
    cli::{CliConnectionOpts, CommandOutput},
    common::{
        boxed_err_to_anyhow, find_actor_id, find_host_id, find_provider_id, get_all_inventories,
        FindIdError, Match,
    },
    config::WashConnectionOptions,
    context::default_timeout_ms,
    id::{validate_contract_id, ServerId},
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

    /// Id of host to stop actor on. If a non-ID is provided, the host will be selected based
    /// on matching the prefix of the ID or the friendly name and will return an error if more than
    /// one host matches. If no host ID is passed, a host will be selected based on whether or not
    /// the actor is running on it. If more than 1 host is running this actor, an error will be
    /// returned with a list of hosts running the actor
    #[clap(long = "host-id")]
    pub host_id: Option<String>,

    /// Actor Id (e.g. the public key for the actor) or a string to match on the prefix of the ID,
    /// or friendly name, or call alias of the actor. If multiple actors are matched, then an error
    /// will be returned with a list of all matching options
    #[clap(name = "actor-id")]
    pub actor_id: String,

    /// Number of actors to stop (DEPRECATED: count is ignored)
    #[clap(long = "count", default_value = "1")]
    #[deprecated(
        since = "0.21.0",
        note = "actor will be stopped regardless of scale, count is now ignored"
    )]
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

    /// Id of host to stop provider on. If a non-ID is provided, the host will be selected based on
    /// matching the prefix of the ID or the friendly name and will return an error if more than one
    /// host matches. If no host ID is passed, a host will be selected based on whether or not the
    /// actor is running on it. If more than 1 host is running this actor, an error will be returned
    /// with a list of hosts running the actor
    #[clap(long = "host-id")]
    pub host_id: Option<String>,

    /// Provider Id (e.g. the public key for the provider) or a string to match on the prefix of the
    /// ID, or friendly name, or call alias of the provider. If multiple providers are matched, then
    /// an error will be returned with a list of all matching options
    #[clap(name = "provider-id")]
    pub provider_id: String,

    /// Capability contract Id of provider.
    #[clap(name = "contract-id")]
    pub contract_id: String,

    // NOTE(thomastaylor312): Since this is a positional argument and is optional, it has to be the
    // last one
    /// Link name of provider. If none is provided, it will default to "default"
    #[clap(name = "link-name", default_value = "default")]
    pub link_name: String,

    /// By default, the command will wait until the provider has been stopped. If this flag is
    /// passed, the command will return immediately after acknowledgement from the host, without
    /// waiting for the provider to stop.
    #[clap(long = "skip-wait")]
    pub skip_wait: bool,
}

#[derive(Debug, Clone, Parser)]
pub struct StopHostCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Id of host to stop. If a non-ID is provided, the host will be selected based on matching the
    /// prefix of the ID or the friendly name and will return an error if more than one host
    /// matches.
    #[clap(name = "host-id")]
    pub host_id: String,

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

    let (provider_id, friendly_name) = find_provider_id(&cmd.provider_id, &client).await?;
    let host_id = if let Some(host_id) = cmd.host_id {
        find_host_id(&host_id, &client).await?.0
    } else {
        find_host_with_provider(&provider_id, &client).await?
    };

    let ack = client
        .stop_provider(
            &host_id,
            &provider_id,
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
        let text = format!(
            "Provider {} stop request received",
            friendly_name.as_deref().unwrap_or(provider_id.as_ref())
        );
        return Ok(CommandOutput::new(
            text.clone(),
            HashMap::from([
                ("result".into(), text.into()),
                ("provider_id".into(), provider_id.to_string().into()),
                ("link_name".into(), cmd.link_name.into()),
                ("contract_id".into(), cmd.contract_id.into()),
                ("host_id".into(), host_id.to_string().into()),
            ]),
        ));
    }

    let event = wait_for_provider_stop_event(
        &mut receiver,
        Duration::from_millis(timeout_ms),
        host_id.to_string(),
        provider_id.to_string(),
    )
    .await?;

    match event {
        FindEventOutcome::Success(ProviderStoppedInfo {
            host_id,
            provider_id,
            link_name,
            contract_id,
        }) => {
            let text = format!(
                "Provider [{}] stopped successfully",
                friendly_name.as_deref().unwrap_or(provider_id.as_ref())
            );
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

    let (actor_id, friendly_name) = find_actor_id(&cmd.actor_id, &client).await?;

    let host_id = if let Some(host_id) = cmd.host_id {
        find_host_id(&host_id, &client).await?.0
    } else {
        find_host_with_actor(&actor_id, &client).await?
    };

    let ActorStoppedInfo { actor_id, host_id } = stop_actor(
        &client,
        &host_id,
        &actor_id,
        None,
        timeout_ms,
        cmd.skip_wait,
    )
    .await?;

    let text = if cmd.skip_wait {
        format!(
            "Request to stop actor {} received",
            friendly_name.as_deref().unwrap_or(actor_id.as_ref())
        )
    } else {
        format!(
            "Actor [{}] stopped",
            friendly_name.as_deref().unwrap_or(actor_id.as_ref())
        )
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

async fn find_host_with_provider(
    provider_id: &str,
    ctl_client: &wasmcloud_control_interface::Client,
) -> Result<ServerId, FindIdError> {
    find_host_with_filter(ctl_client, |inv| {
        inv.providers
            .into_iter()
            .any(|prov| prov.id == provider_id)
            .then_some((inv.host_id, inv.friendly_name))
            .and_then(|(id, friendly_name)| id.parse().ok().map(|i| (i, friendly_name)))
    })
    .await
}

pub(crate) async fn find_host_with_actor(
    actor_id: &str,
    ctl_client: &wasmcloud_control_interface::Client,
) -> Result<ServerId, FindIdError> {
    find_host_with_filter(ctl_client, |inv| {
        inv.actors
            .into_iter()
            .any(|actor| actor.id == actor_id)
            .then_some((inv.host_id, inv.friendly_name))
            .and_then(|(id, friendly_name)| id.parse().ok().map(|i| (i, friendly_name)))
    })
    .await
}

async fn find_host_with_filter<F>(
    ctl_client: &wasmcloud_control_interface::Client,
    filter: F,
) -> Result<ServerId, FindIdError>
where
    F: FnMut(HostInventory) -> Option<(ServerId, String)>,
{
    let inventories = get_all_inventories(ctl_client).await?;
    let all_matching = inventories
        .into_iter()
        .filter_map(filter)
        .collect::<Vec<(ServerId, String)>>();

    if all_matching.is_empty() {
        Err(FindIdError::NoMatches)
    } else if all_matching.len() > 1 {
        Err(FindIdError::MultipleMatches(
            all_matching
                .into_iter()
                .map(|(id, friendly_name)| Match {
                    id: id.into_string(),
                    friendly_name: Some(friendly_name),
                })
                .collect(),
        ))
    } else {
        // SAFETY: We know there is exactly one match at this point
        Ok(all_matching.into_iter().next().unwrap().0)
    }
}
