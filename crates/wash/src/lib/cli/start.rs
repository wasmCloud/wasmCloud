use std::collections::{BTreeMap, HashMap};

use anyhow::{bail, Context, Result};
use clap::Parser;
use tokio::time::Duration;

use crate::lib::cli::{input_vec_to_hashmap, CliConnectionOpts, CommandOutput};
use crate::lib::common::{boxed_err_to_anyhow, find_host_id};
use crate::lib::component::{scale_component, ComponentScaledInfo, ScaleComponentArgs};
use crate::lib::config::{
    WashConnectionOptions, DEFAULT_NATS_TIMEOUT_MS, DEFAULT_START_COMPONENT_TIMEOUT_MS,
    DEFAULT_START_PROVIDER_TIMEOUT_MS,
};
use crate::lib::context::default_timeout_ms;
use crate::lib::wait::{wait_for_provider_start_event, FindEventOutcome, ProviderStartedInfo};

use super::validate_component_id;

#[derive(Debug, Clone, Parser)]
pub enum StartCommand {
    /// Launch a component in a host
    #[clap(name = "component")]
    Component(StartComponentCommand),

    /// Launch a provider in a host
    #[clap(name = "provider")]
    Provider(StartProviderCommand),
}

#[derive(Debug, Clone, Parser)]
pub struct StartComponentCommand {
    #[clap(flatten)]
    pub opts: CliConnectionOpts,

    /// Id of host or a string to match on the friendly name of a host. if omitted the component will be
    /// auctioned in the lattice to find a suitable host. If a string is supplied to match against,
    /// then the matching host ID will be used. If more than one host matches, then an error will be
    /// returned
    #[clap(long = "host-id")]
    pub host_id: Option<String>,

    /// Component reference, e.g. the absolute file path or OCI URL.
    #[clap(name = "component-ref")]
    pub component_ref: String,

    /// Unique ID to use for the component
    #[clap(name = "component-id", value_parser = validate_component_id)]
    pub component_id: String,

    /// Maximum number of instances this component can run concurrently.
    #[clap(
        long = "max-instances",
        alias = "max-concurrent",
        alias = "max",
        alias = "count",
        default_value_t = 1
    )]
    pub max_instances: u32,

    /// Constraints for component auction in the form of "label=value". If host-id is supplied, this list is ignored
    #[clap(short = 'c', long = "constraint", name = "constraints")]
    pub constraints: Option<Vec<String>>,

    /// Timeout to await an auction response, defaults to 2000 milliseconds
    #[clap(long = "auction-timeout-ms", default_value_t = default_timeout_ms())]
    pub auction_timeout_ms: u64,

    /// By default, the command will wait until the component has been started.
    /// If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the component to start.
    /// If this flag is omitted, the timeout will be adjusted to 5 seconds to account for component download times
    #[clap(long = "skip-wait")]
    pub skip_wait: bool,

    /// List of named configuration to apply to the component, may be empty
    #[clap(long = "config")]
    pub config: Vec<String>,
}

/// Utility function for resolving component and provider references
pub(crate) async fn resolve_ref(s: impl AsRef<str>) -> Result<String> {
    let resolved = match s.as_ref() {
        s if s.starts_with('/') => {
            format!("file://{}", &s) // prefix with file:// if it's an absolute path
        }
        s if tokio::fs::try_exists(s).await.is_ok_and(|exists| exists) => {
            format!(
                "file://{}",
                tokio::fs::canonicalize(&s)
                    .await
                    .with_context(|| format!("failed to resolve absolute path: {s}"))?
                    .display()
            )
        }
        // If a URI-formatted relative path was provided, resolve it
        s if s.starts_with("file://")
            && tokio::fs::try_exists(s.split_at(7).1)
                .await
                .is_ok_and(|exists| exists) =>
        {
            format!(
                "file://{}",
                tokio::fs::canonicalize(s.split_at(7).1)
                    .await
                    .with_context(|| format!("failed to resolve absolute path: {s}"))?
                    .display()
            )
        }
        // For all other cases, just take the provided string
        s => s.to_string(),
    };
    Ok(resolved)
}

pub async fn handle_start_component(cmd: StartComponentCommand) -> Result<CommandOutput> {
    // If timeout isn't supplied, override with a longer timeout for starting component
    let timeout_ms = if cmd.opts.timeout_ms == DEFAULT_NATS_TIMEOUT_MS {
        DEFAULT_START_COMPONENT_TIMEOUT_MS
    } else {
        cmd.opts.timeout_ms
    };
    let client = <CliConnectionOpts as TryInto<WashConnectionOptions>>::try_into(cmd.opts)?
        .into_ctl_client(Some(cmd.auction_timeout_ms))
        .await?;

    let component_ref = resolve_ref(&cmd.component_ref).await?;

    let host = if let Some(host) = cmd.host_id {
        find_host_id(&host, &client).await?.0
    } else {
        let suitable_hosts = client
            .perform_component_auction(
                &component_ref,
                &cmd.component_id,
                BTreeMap::from_iter(input_vec_to_hashmap(cmd.constraints.unwrap_or_default())?),
            )
            .await
            .map_err(boxed_err_to_anyhow)
            .with_context(|| {
                format!(
                    "Failed to auction component {} to hosts in lattice",
                    &component_ref
                )
            })?;
        if suitable_hosts.is_empty() {
            bail!("No suitable hosts found for component {}", component_ref);
        } else {
            let acks = suitable_hosts
                .into_iter()
                .filter_map(wasmcloud_control_interface::CtlResponse::into_data)
                .collect::<Vec<_>>();
            let ack = acks.first().context("No suitable hosts found")?;
            ack.host_id()
                .parse()
                .with_context(|| format!("Failed to parse host id: {}", ack.host_id()))?
        }
    };

    // Start the component
    let ComponentScaledInfo {
        host_id,
        component_ref,
        component_id,
    } = scale_component(ScaleComponentArgs {
        client: &client,
        host_id: &host,
        component_ref: &component_ref,
        component_id: &cmd.component_id,
        max_instances: cmd.max_instances,
        skip_wait: cmd.skip_wait,
        timeout_ms: Some(timeout_ms),
        annotations: None,
        config: cmd.config,
    })
    .await?;

    let text = if cmd.skip_wait {
        format!("Start component [{component_ref}] request received on host [{host_id}]",)
    } else {
        format!("Component [{component_id}] (ref: [{component_ref}]) started on host [{host_id}]",)
    };

    Ok(CommandOutput::new(
        text.clone(),
        HashMap::from([
            ("result".into(), text.into()),
            ("component_ref".into(), component_ref.into()),
            ("component_id".into(), component_id.into()),
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

    /// Unique provider ID to use for the provider
    #[clap(name = "provider-id", value_parser = validate_component_id)]
    pub provider_id: String,

    /// Link name of provider
    #[clap(short = 'l', long = "link-name", default_value = "default")]
    pub link_name: String,

    /// Constraints for provider auction in the form of "label=value". If host-id is supplied, this list is ignored
    #[clap(short = 'c', long = "constraint", name = "constraints")]
    pub constraints: Option<Vec<String>>,

    /// Timeout to await an auction response, defaults to 2000 milliseconds
    #[clap(long = "auction-timeout-ms", default_value_t = default_timeout_ms())]
    pub auction_timeout_ms: u64,

    /// List of named configuration to apply to the provider, may be empty
    #[clap(long = "config")]
    pub config: Vec<String>,

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

    // Attempt to parse the provider_ref from strings that may look like paths or be OCI references
    let provider_ref = resolve_ref(&cmd.provider_ref).await?;

    let host = if let Some(host) = cmd.host_id {
        find_host_id(&host, &client).await?.0
    } else {
        let suitable_hosts = client
            .perform_provider_auction(
                &provider_ref,
                &cmd.link_name,
                BTreeMap::from_iter(input_vec_to_hashmap(cmd.constraints.unwrap_or_default())?),
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
            let acks = suitable_hosts
                .into_iter()
                .filter_map(wasmcloud_control_interface::CtlResponse::into_data)
                .collect::<Vec<_>>();
            let ack = acks.first().context("No suitable hosts found")?;
            ack.host_id()
                .parse()
                .with_context(|| format!("Failed to parse host id: {}", ack.host_id()))?
        }
    };

    let mut receiver = client
        .events_receiver(vec![
            "provider_started".to_string(),
            "provider_start_failed".to_string(),
        ])
        .await
        .map_err(boxed_err_to_anyhow)
        .context("Failed to get lattice event channel")?;

    let ack = client
        .start_provider(&host, &provider_ref, &cmd.provider_id, None, cmd.config)
        .await
        .map_err(boxed_err_to_anyhow)
        .with_context(|| {
            format!(
                "Failed to start provider {} on host {:?}",
                &cmd.provider_id, &host
            )
        })?;

    if !ack.succeeded() {
        bail!("Start provider ack not accepted: {}", ack.message());
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
