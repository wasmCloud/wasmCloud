use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use tokio::time::Duration;

use crate::{
    actor::{start_actor, ActorStartedInfo, StartActorArgs},
    cli::{labels_vec_to_hashmap, CliConnectionOpts, CommandOutput},
    common::{boxed_err_to_anyhow, find_host_id},
    config::{
        WashConnectionOptions, DEFAULT_NATS_TIMEOUT_MS, DEFAULT_START_ACTOR_TIMEOUT_MS,
        DEFAULT_START_PROVIDER_TIMEOUT_MS,
    },
    context::default_timeout_ms,
    wait::{wait_for_provider_start_event, FindEventOutcome, ProviderStartedInfo},
};

#[derive(Debug, Clone, Parser)]
pub enum StartCommand {
    /// Launch an actor in a host
    #[clap(name = "actor")]
    Actor(StartActorCommand),

    /// Launch a provider in a host
    #[clap(name = "provider")]
    Provider(StartProviderCommand),
}

#[derive(Debug, Clone, Parser)]
pub struct StartActorCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Id of host or a string to match on the friendly name of a host. if omitted the actor will be
    /// auctioned in the lattice to find a suitable host. If a string is supplied to match against,
    /// then the matching host ID will be used. If more than one host matches, then an error will be
    /// returned
    #[clap(long = "host-id")]
    pub host_id: Option<String>,

    /// Actor reference, e.g. the OCI URL for the actor.
    #[clap(name = "actor-ref")]
    pub actor_ref: String,

    /// Maximum number of instances this actor can run concurrently. Setting this value to 0 means there is no maximum.
    #[clap(
        long = "max-concurrent",
        alias = "max",
        alias = "count",
        default_value = "1"
    )]
    pub max_concurrent: u16,

    /// Constraints for actor auction in the form of "label=value". If host-id is supplied, this list is ignored
    #[clap(short = 'c', long = "constraint", name = "constraints")]
    pub constraints: Option<Vec<String>>,

    /// Timeout to await an auction response, defaults to 2000 milliseconds
    #[clap(long = "auction-timeout-ms", default_value_t = default_timeout_ms())]
    pub auction_timeout_ms: u64,

    /// By default, the command will wait until the actor has been started.
    /// If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the actor to start.
    /// If this flag is omitted, the timeout will be adjusted to 5 seconds to account for actor download times
    #[clap(long = "skip-wait")]
    pub skip_wait: bool,
}

pub async fn handle_start_actor(cmd: StartActorCommand) -> Result<CommandOutput> {
    // If timeout isn't supplied, override with a longer timeout for starting actor
    let timeout_ms = if cmd.opts.timeout_ms == DEFAULT_NATS_TIMEOUT_MS {
        DEFAULT_START_ACTOR_TIMEOUT_MS
    } else {
        cmd.opts.timeout_ms
    };
    let client = <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?
        .into_ctl_client(Some(cmd.auction_timeout_ms))
        .await?;

    let actor_ref = if cmd.actor_ref.starts_with('/') {
        format!("file://{}", &cmd.actor_ref) // prefix with file:// if it's an absolute path
    } else {
        cmd.actor_ref.to_string()
    };

    let host = match cmd.host_id {
        Some(host) => find_host_id(&host, &client).await?.0,
        None => {
            let suitable_hosts = client
                .perform_actor_auction(
                    &actor_ref,
                    labels_vec_to_hashmap(cmd.constraints.unwrap_or_default())?,
                )
                .await
                .map_err(boxed_err_to_anyhow)
                .with_context(|| {
                    format!("Failed to auction actor {} to hosts in lattice", &actor_ref)
                })?;
            if suitable_hosts.is_empty() {
                bail!("No suitable hosts found for actor {}", actor_ref);
            } else {
                suitable_hosts[0].host_id.parse().with_context(|| {
                    format!("Failed to parse host id: {}", suitable_hosts[0].host_id)
                })?
            }
        }
    };

    // Start the actor
    let ActorStartedInfo {
        host_id,
        actor_ref,
        actor_id,
    } = start_actor(StartActorArgs {
        ctl_client: &client,
        host_id: &host,
        actor_ref: &actor_ref,
        count: cmd.max_concurrent,
        skip_wait: cmd.skip_wait,
        timeout_ms: Some(timeout_ms),
    })
    .await?;

    let text = if cmd.skip_wait {
        format!(
            "Start actor [{}] request received on host [{}]",
            actor_ref, host_id
        )
    } else {
        format!(
            "Actor [{}] (ref: [{}]) started on host [{}]",
            actor_id.clone().unwrap_or("<unknown>".into()),
            &actor_ref,
            &host_id
        )
    };

    Ok(CommandOutput::new(
        text.clone(),
        HashMap::from([
            ("result".into(), text.into()),
            ("actor_ref".into(), actor_ref.into()),
            ("actor_id".into(), actor_id.into()),
            ("host_id".into(), host_id.into()),
        ]),
    ))
}

#[derive(Debug, Clone, Parser)]
pub struct StartProviderCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Id of host or a string to match on the friendly name of a host. if omitted the provider will
    /// be auctioned in the lattice to find a suitable host. If a string is supplied to match
    /// against, then the matching host ID will be used. If more than one host matches, then an
    /// error will be returned
    #[clap(long = "host-id")]
    pub host_id: Option<String>,

    /// Provider reference, e.g. the OCI URL for the provider
    #[clap(name = "provider-ref")]
    pub provider_ref: String,

    /// Link name of provider
    #[clap(short = 'l', long = "link-name", default_value = "default")]
    pub link_name: String,

    /// Constraints for provider auction in the form of "label=value". If host-id is supplied, this list is ignored
    #[clap(short = 'c', long = "constraint", name = "constraints")]
    pub constraints: Option<Vec<String>>,

    /// Timeout to await an auction response, defaults to 2000 milliseconds
    #[clap(long = "auction-timeout-ms", default_value_t = default_timeout_ms())]
    pub auction_timeout_ms: u64,

    /// Path to provider configuration JSON file
    #[clap(long = "config-json")]
    pub config_json: Option<PathBuf>,

    /// By default, the command will wait until the provider has been started.
    /// If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the provider to start.
    /// If this flag is omitted, the timeout will be adjusted to 30 seconds to account for provider download times
    #[clap(long = "skip-wait")]
    pub skip_wait: bool,
}

pub async fn handle_start_provider(cmd: StartProviderCommand) -> Result<CommandOutput> {
    // If timeout isn't supplied, override with a longer timeout for starting provider
    let timeout_ms = if cmd.opts.timeout_ms == DEFAULT_NATS_TIMEOUT_MS {
        DEFAULT_START_PROVIDER_TIMEOUT_MS
    } else {
        cmd.opts.timeout_ms
    };
    let client = <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?
        .into_ctl_client(Some(cmd.auction_timeout_ms))
        .await?;

    let provider_ref = if cmd.provider_ref.starts_with('/') {
        format!("file://{}", &cmd.provider_ref) // prefix with file:// if it's an absolute path
    } else {
        cmd.provider_ref.to_string()
    };

    let host = match cmd.host_id {
        Some(host) => find_host_id(&host, &client).await?.0,
        None => {
            let suitable_hosts = client
                .perform_provider_auction(
                    &provider_ref,
                    &cmd.link_name,
                    labels_vec_to_hashmap(cmd.constraints.unwrap_or_default())?,
                )
                .await
                .map_err(boxed_err_to_anyhow)
                .with_context(|| {
                    format!(
                        "Failed to auction provider {} with link name {} to hosts in lattice",
                        &provider_ref, &cmd.link_name
                    )
                })?;
            if suitable_hosts.is_empty() {
                bail!("No suitable hosts found for provider {}", provider_ref);
            } else {
                suitable_hosts[0].host_id.parse().with_context(|| {
                    format!("Failed to parse host id: {}", suitable_hosts[0].host_id)
                })?
            }
        }
    };

    let config_json = if let Some(config_path) = cmd.config_json {
        let config_str = match std::fs::read_to_string(&config_path) {
            Ok(s) => s,
            Err(e) => bail!("Error reading provider configuration: {}", e),
        };
        match serde_json::from_str::<serde_json::Value>(&config_str) {
            Ok(_v) => Some(config_str),
            _ => bail!(
                "Configuration path provided but was invalid JSON: {}",
                config_path.display()
            ),
        }
    } else {
        None
    };

    let mut receiver = client
        .events_receiver()
        .await
        .map_err(boxed_err_to_anyhow)
        .context("Failed to get lattice event channel")?;

    let ack = client
        .start_provider(
            &host,
            &provider_ref,
            Some(cmd.link_name.clone()),
            None,
            config_json.clone(),
        )
        .await
        .map_err(boxed_err_to_anyhow)
        .with_context(|| {
            format!(
                "Failed to start provider {} on host {:?} with link name {} and configuration {:?}",
                &provider_ref, &host, &cmd.link_name, &config_json
            )
        })?;

    if !ack.accepted {
        bail!("Start provider ack not accepted: {}", ack.error);
    }

    if cmd.skip_wait {
        let text = format!("Start provider request received: {}", &provider_ref);
        return Ok(CommandOutput::new(
            text.clone(),
            HashMap::from([
                ("result".into(), text.into()),
                ("provider_ref".into(), provider_ref.into()),
                ("link_name".into(), cmd.link_name.into()),
                ("host_id".into(), host.to_string().into()),
            ]),
        ));
    }

    let event = wait_for_provider_start_event(
        &mut receiver,
        Duration::from_millis(timeout_ms),
        host.to_string(),
        provider_ref.clone(),
    )
    .await
    .with_context(|| {
        format!(
            "Timed out waiting for start event for provider {} on host {}",
            &provider_ref, &host
        )
    })?;

    match event {
        FindEventOutcome::Success(ProviderStartedInfo {
            provider_id,
            provider_ref,
            host_id,
            contract_id,
            link_name,
        }) => {
            let text = format!(
                "Provider [{}] (ref: [{}]) started on host [{}]",
                &provider_id, &provider_ref, &host_id
            );
            Ok(CommandOutput::new(
                text.clone(),
                HashMap::from([
                    ("result".into(), text.into()),
                    ("provider_ref".into(), provider_ref.into()),
                    ("provider_id".into(), provider_id.into()),
                    ("link_name".into(), link_name.into()),
                    ("contract_id".into(), contract_id.into()),
                    ("host_id".into(), host_id.into()),
                ]),
            ))
        }
        FindEventOutcome::Failure(err) => Err(err).with_context(|| {
            format!(
                "Failed starting provider {} on host {}",
                &provider_ref, &host
            )
        }),
    }
}
