use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Parser;
use tokio::time::Duration;

use crate::{
    cli::{labels_vec_to_hashmap, CliConnectionOpts, CommandOutput},
    common::boxed_err_to_anyhow,
    config::{
        WashConnectionOptions, DEFAULT_NATS_TIMEOUT_MS, DEFAULT_START_ACTOR_TIMEOUT_MS,
        DEFAULT_START_PROVIDER_TIMEOUT_MS,
    },
    context::default_timeout_ms,
    id::ServerId,
    wait::{wait_for_actor_start_event, wait_for_provider_start_event, FindEventOutcome},
};

#[derive(Debug, Clone, Parser)]
pub struct StartActorCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Id of host, if omitted the actor will be auctioned in the lattice to find a suitable host
    #[clap(long = "host-id", name = "host-id", value_parser)]
    pub host_id: Option<ServerId>,

    /// Actor reference, e.g. the OCI URL for the actor.
    #[clap(name = "actor-ref")]
    pub actor_ref: String,

    /// Number of actors to start
    #[clap(long = "count", default_value = "1")]
    pub count: u16,

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

pub async fn start_actor(cmd: StartActorCommand) -> Result<CommandOutput> {
    // If timeout isn't supplied, override with a longer timeout for starting actor
    let timeout_ms = if cmd.opts.timeout_ms == DEFAULT_NATS_TIMEOUT_MS {
        DEFAULT_START_ACTOR_TIMEOUT_MS
    } else {
        cmd.opts.timeout_ms
    };
    let client = <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?
        .into_ctl_client(Some(cmd.auction_timeout_ms))
        .await?;

    let host = match cmd.host_id {
        Some(host) => host,
        None => {
            let suitable_hosts = client
                .perform_actor_auction(
                    &cmd.actor_ref,
                    labels_vec_to_hashmap(cmd.constraints.unwrap_or_default())?,
                )
                .await
                .map_err(boxed_err_to_anyhow)
                .with_context(|| {
                    format!(
                        "Failed to auction actor {} to hosts in lattice",
                        &cmd.actor_ref
                    )
                })?;
            if suitable_hosts.is_empty() {
                bail!("No suitable hosts found for actor {}", cmd.actor_ref);
            } else {
                suitable_hosts[0].host_id.parse().with_context(|| {
                    format!("Failed to parse host id: {}", suitable_hosts[0].host_id)
                })?
            }
        }
    };

    let mut receiver = client
        .events_receiver()
        .await
        .map_err(boxed_err_to_anyhow)
        .context("Failed to get lattice event channel")?;

    let ack = client
        .start_actor(&host.to_string(), &cmd.actor_ref, cmd.count, None)
        .await
        .map_err(boxed_err_to_anyhow)
        .with_context(|| format!("Failed to start actor: {}", &cmd.actor_ref))?;

    if !ack.accepted {
        bail!("Start actor ack not accepted: {}", ack.error);
    }

    if cmd.skip_wait {
        return Ok(CommandOutput::from_key_and_text(
            "result",
            format!(
                "Start actor request received: {}, host: {}",
                &cmd.actor_ref, &host
            ),
        ));
    }

    let event = wait_for_actor_start_event(
        &mut receiver,
        Duration::from_millis(timeout_ms),
        host.to_string(),
        cmd.actor_ref.clone(),
    )
    .await
    .with_context(|| {
        format!(
            "Timed out waitng for start event for actor {} on host {}",
            &cmd.actor_ref, &host
        )
    })?;

    match event {
        FindEventOutcome::Success(_) => Ok(CommandOutput::from_key_and_text(
            "result",
            format!("Actor {} started on host {}", cmd.actor_ref, host),
        )),
        FindEventOutcome::Failure(err) => Err(err)
            .with_context(|| format!("Failed to start actor {} on host {}", &cmd.actor_ref, &host)),
    }
}

#[derive(Debug, Clone, Parser)]
pub struct StartProviderCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Id of host, if omitted the provider will be auctioned in the lattice to find a suitable host
    #[clap(long = "host-id", name = "host-id", value_parser)]
    pub host_id: Option<ServerId>,

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

pub async fn start_provider(cmd: StartProviderCommand) -> Result<CommandOutput> {
    // If timeout isn't supplied, override with a longer timeout for starting provider
    let timeout_ms = if cmd.opts.timeout_ms == DEFAULT_NATS_TIMEOUT_MS {
        DEFAULT_START_PROVIDER_TIMEOUT_MS
    } else {
        cmd.opts.timeout_ms
    };
    let client = <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?
        .into_ctl_client(Some(cmd.auction_timeout_ms))
        .await?;

    let host = match cmd.host_id {
        Some(host) => host,
        None => {
            let suitable_hosts = client
                .perform_provider_auction(
                    &cmd.provider_ref,
                    &cmd.link_name,
                    labels_vec_to_hashmap(cmd.constraints.unwrap_or_default())?,
                )
                .await
                .map_err(boxed_err_to_anyhow)
                .with_context(|| {
                    format!(
                        "Failed to auction provider {} with link name {} to hosts in lattice",
                        &cmd.provider_ref, &cmd.link_name
                    )
                })?;
            if suitable_hosts.is_empty() {
                bail!("No suitable hosts found for provider {}", cmd.provider_ref);
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
            &host.to_string(),
            &cmd.provider_ref,
            Some(cmd.link_name.clone()),
            None,
            config_json.clone(),
        )
        .await
        .map_err(boxed_err_to_anyhow)
        .with_context(|| {
            format!(
                "Failed to start provider {} on host {:?} with link name {} and configuration {:?}",
                &cmd.provider_ref, &host, &cmd.link_name, &config_json
            )
        })?;

    if !ack.accepted {
        bail!("Start provider ack not accepted: {}", ack.error);
    }

    if cmd.skip_wait {
        return Ok(CommandOutput::from_key_and_text(
            "result",
            format!("Start provider request received: {}", &cmd.provider_ref),
        ));
    }

    let event = wait_for_provider_start_event(
        &mut receiver,
        Duration::from_millis(timeout_ms),
        host.to_string(),
        cmd.provider_ref.clone(),
    )
    .await
    .with_context(|| {
        format!(
            "Timed out waiting for start event for provider {} on host {}",
            &cmd.provider_ref, &host
        )
    })?;

    match event {
        FindEventOutcome::Success(_) => Ok(CommandOutput::from_key_and_text(
            "result",
            format!("Provider {} started on host {}", cmd.provider_ref, host),
        )),
        FindEventOutcome::Failure(err) => Err(err).with_context(|| {
            format!(
                "Failed starting provider {} on host {}",
                &cmd.provider_ref, &host
            )
        }),
    }
}
