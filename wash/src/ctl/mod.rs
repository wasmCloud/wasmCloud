extern crate wasmcloud_control_interface;

use crate::{
    appearance::spinner::Spinner,
    ctl::manifest::HostManifest,
    ctx::{context_dir, get_default_context, load_context},
    id::{ModuleId, ServerId, ServiceId},
    util::{
        convert_error, default_timeout_ms, labels_vec_to_hashmap, validate_contract_id,
        CommandOutput, OutputKind, DEFAULT_LATTICE_PREFIX, DEFAULT_NATS_HOST, DEFAULT_NATS_PORT,
        DEFAULT_NATS_TIMEOUT_MS, DEFAULT_START_PROVIDER_TIMEOUT_MS,
    },
};
use anyhow::{bail, Result};
use clap::{AppSettings, ArgEnum, Args, Parser, Subcommand};
pub(crate) use output::*;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use wasmcloud_control_interface::{
    Client as CtlClient, CtlOperationAck, GetClaimsResponse, Host, HostInventory,
    LinkDefinitionList,
};

use self::wait::{
    wait_for_actor_start_event, wait_for_actor_stop_event, wait_for_provider_start_event,
    wait_for_provider_stop_event, FindEventOutcome,
};

mod manifest;
mod output;
mod wait;

// default start actor command starts with one actor
const ONE_ACTOR: u16 = 1;

#[derive(Args, Debug, Clone)]
//#[clap(PARENT_APP_ATTRIBUTE)]
pub(crate) struct ConnectionOpts {
    /// CTL Host for connection, defaults to 127.0.0.1 for local nats
    #[clap(short = 'r', long = "ctl-host", env = "WASMCLOUD_CTL_HOST")]
    ctl_host: Option<String>,

    /// CTL Port for connections, defaults to 4222 for local nats
    #[clap(short = 'p', long = "ctl-port", env = "WASMCLOUD_CTL_PORT")]
    ctl_port: Option<String>,

    /// JWT file for CTL authentication. Must be supplied with ctl_seed.
    #[clap(long = "ctl-jwt", env = "WASMCLOUD_CTL_JWT", hide_env_values = true)]
    ctl_jwt: Option<String>,

    /// Seed file or literal for CTL authentication. Must be supplied with ctl_jwt.
    #[clap(long = "ctl-seed", env = "WASMCLOUD_CTL_SEED", hide_env_values = true)]
    ctl_seed: Option<String>,

    /// Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt.
    /// See https://docs.nats.io/developing-with-nats/security/creds for details.
    #[clap(long = "ctl-credsfile", env = "WASH_CTL_CREDS", hide_env_values = true)]
    ctl_credsfile: Option<PathBuf>,

    /// Lattice prefix for wasmcloud control interface, defaults to "default"
    #[clap(short = 'x', long = "lattice-prefix", env = "WASMCLOUD_LATTICE_PREFIX")]
    lattice_prefix: Option<String>,

    /// Timeout length to await a control interface response, defaults to 2000 milliseconds
    #[clap(
        short = 't',
        long = "ack-timeout-ms",
        default_value_t = default_timeout_ms(),
        env = "WASMCLOUD_CTL_TIMEOUT_MS"
    )]
    ack_timeout_ms: u64,

    /// Path to a context with values to use for CTL connection and authentication
    #[clap(long = "context")]
    pub(crate) context: Option<PathBuf>,
}

impl Default for ConnectionOpts {
    fn default() -> Self {
        ConnectionOpts {
            ctl_host: Some(DEFAULT_NATS_HOST.to_string()),
            ctl_port: Some(DEFAULT_NATS_PORT.to_string()),
            ctl_jwt: None,
            ctl_seed: None,
            ctl_credsfile: None,
            lattice_prefix: Some(DEFAULT_LATTICE_PREFIX.to_string()),
            ack_timeout_ms: DEFAULT_NATS_TIMEOUT_MS,
            context: None,
        }
    }
}

#[derive(ArgEnum, Debug, Clone, Subcommand)]
pub(crate) enum CtlCliCommand {
    /// Retrieves information about the lattice
    #[clap(name = "get", subcommand)]
    Get(GetCommand),

    /// Link an actor and a provider
    #[clap(name = "link", subcommand)]
    Link(LinkCommand),

    /// Start an actor or a provider
    #[clap(name = "start", subcommand)]
    Start(StartCommand),

    /// Stop an actor, provider, or host
    #[clap(name = "stop", subcommand)]
    Stop(StopCommand),

    /// Update an actor running in a host to a new actor
    #[clap(name = "update", subcommand)]
    Update(UpdateCommand),

    /// Apply a manifest file to a target host
    #[clap(name = "apply")]
    Apply(ApplyCommand),

    #[clap(name = "scale", subcommand)]
    Scale(ScaleCommand),
}

#[derive(Args, Debug, Clone)]
pub(crate) struct ApplyCommand {
    /// Public key of the target host for the manifest application
    #[clap(name = "host-key", parse(try_from_str))]
    pub(crate) host_key: ServerId,

    /// Path to the manifest file. Note that all the entries in this file are imperative instructions, and all actor and provider references MUST be valid OCI references.
    #[clap(name = "path")]
    pub(crate) path: String,

    /// Expand environment variables using substitution syntax within the manifest file
    #[clap(name = "expand-env", short = 'e', long = "expand-env")]
    pub(crate) expand_env: bool,

    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum GetCommand {
    /// Query lattice for running hosts
    #[clap(name = "hosts")]
    Hosts(GetHostsCommand),

    /// Query a single host for its inventory of labels, actors and providers
    #[clap(name = "inventory")]
    HostInventory(GetHostInventoryCommand),

    /// Query lattice for its claims cache
    #[clap(name = "claims")]
    Claims(GetClaimsCommand),
}

#[derive(Debug, Clone, Parser)]
pub(crate) enum LinkCommand {
    /// Query established links
    #[clap(name = "query")]
    Query(LinkQueryCommand),

    /// Establish a link definition
    #[clap(name = "put")]
    Put(LinkPutCommand),

    /// Delete a link definition
    #[clap(name = "del")]
    Del(LinkDelCommand),
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct LinkQueryCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct LinkDelCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,

    /// Public key ID of actor
    #[clap(name = "actor-id", parse(try_from_str))]
    pub(crate) actor_id: ModuleId,

    /// Capability contract ID between actor and provider
    #[clap(name = "contract-id")]
    pub(crate) contract_id: String,

    /// Link name, defaults to "default"
    #[clap(short = 'l', long = "link-name")]
    pub(crate) link_name: Option<String>,
}

#[derive(Parser, Debug, Clone)]
#[clap(
    override_usage = "wash ctl link put --link-name <LINK_NAME> [OPTIONS] <actor-id> <provider-id> <contract-id> [values]..."
)]
pub(crate) struct LinkPutCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,

    /// Public key ID of actor
    #[clap(name = "actor-id", parse(try_from_str))]
    pub(crate) actor_id: ModuleId,

    /// Public key ID of provider
    #[clap(name = "provider-id", parse(try_from_str))]
    pub(crate) provider_id: ServiceId,

    /// Capability contract ID between actor and provider
    #[clap(name = "contract-id")]
    pub(crate) contract_id: String,

    /// Link name, defaults to "default"
    #[clap(short = 'l', long = "link-name")]
    pub(crate) link_name: Option<String>,

    /// Environment values to provide alongside link
    #[clap(name = "values")]
    pub(crate) values: Vec<String>,
}

#[derive(Debug, Clone, Parser)]
pub(crate) enum StartCommand {
    /// Launch an actor in a host
    #[clap(name = "actor")]
    Actor(StartActorCommand),

    /// Launch a provider in a host
    #[clap(name = "provider")]
    Provider(StartProviderCommand),
}

#[derive(Debug, Clone, Parser)]
pub(crate) enum StopCommand {
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
pub(crate) enum UpdateCommand {
    /// Update an actor running in a host
    #[clap(name = "actor")]
    Actor(UpdateActorCommand),
}

#[derive(Debug, Clone, Parser)]
pub(crate) enum ScaleCommand {
    /// Scale an actor running in a host
    #[clap(name = "actor")]
    Actor(ScaleActorCommand),
}

#[derive(Debug, Clone, Parser)]
pub struct ScaleActorCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,

    /// Id of host
    #[clap(name = "host-id", parse(try_from_str))]
    host_id: ServerId,

    /// Actor Id, e.g. the public key for the actor
    #[clap(name = "actor-id", parse(try_from_str))]
    pub(crate) actor_id: ModuleId,

    /// Actor reference, e.g. the OCI URL for the actor.
    #[clap(name = "actor-ref")]
    pub(crate) actor_ref: String,

    /// Number of actors to scale to.
    #[clap(short = 'c', long = "count", default_value = "1")]
    pub count: u16,

    /// Optional set of annotations used to describe the nature of this actor scale command.
    /// For example, autonomous agents may wish to “tag” scale requests as part of a given deployment
    #[clap(short = 'a', long = "annotations")]
    pub annotations: Vec<String>,
}

#[derive(Debug, Clone, Parser)]
pub(crate) struct GetHostsCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Debug, Clone, Parser)]
pub(crate) struct GetHostInventoryCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,

    /// Id of host
    #[clap(name = "host-id", parse(try_from_str))]
    pub(crate) host_id: ServerId,
}

#[derive(Debug, Clone, Parser)]
pub(crate) struct GetClaimsCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,
}

#[derive(Debug, Clone, Parser)]
#[clap(setting(AppSettings::DisableHelpFlag))]
pub(crate) struct StartActorCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,

    /// Id of host, if omitted the actor will be auctioned in the lattice to find a suitable host
    #[clap(short = 'h', long = "host-id", name = "host-id", parse(try_from_str))]
    pub(crate) host_id: Option<ServerId>,

    /// Actor reference, e.g. the OCI URL for the actor.
    #[clap(name = "actor-ref")]
    pub(crate) actor_ref: String,

    /// Number of actors to start
    #[clap(long = "count", default_value = "1")]
    pub(crate) count: u16,

    /// Constraints for actor auction in the form of "label=value". If host-id is supplied, this list is ignored
    #[clap(short = 'c', long = "constraint", name = "constraints")]
    constraints: Option<Vec<String>>,

    /// Timeout to await an auction response, defaults to 2000 milliseconds
    #[clap(long = "auction-timeout-ms", default_value_t = default_timeout_ms())]
    auction_timeout_ms: u64,

    /// By default, the command will wait until the actor has been started.
    /// If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the actor to start.
    #[clap(long = "skip-wait")]
    skip_wait: bool,

    /// Timeout to await an actor start, defaults to 3000 milliseconds.
    #[clap(long = "timeout-ms", default_value_t = 3000)]
    timeout_ms: u64,
}

#[derive(Debug, Clone, Parser)]
#[clap(setting(AppSettings::DisableHelpFlag))]
pub(crate) struct StartProviderCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,

    /// Id of host, if omitted the provider will be auctioned in the lattice to find a suitable host
    #[clap(short = 'h', long = "host-id", name = "host-id", parse(try_from_str))]
    host_id: Option<ServerId>,

    /// Provider reference, e.g. the OCI URL for the provider
    #[clap(name = "provider-ref")]
    pub(crate) provider_ref: String,

    /// Link name of provider
    #[clap(short = 'l', long = "link-name", default_value = "default")]
    pub(crate) link_name: String,

    /// Constraints for provider auction in the form of "label=value". If host-id is supplied, this list is ignored
    #[clap(short = 'c', long = "constraint", name = "constraints")]
    constraints: Option<Vec<String>>,

    /// Timeout to await an auction response, defaults to 2000 milliseconds
    #[clap(long = "auction-timeout-ms", default_value_t = default_timeout_ms())]
    auction_timeout_ms: u64,

    /// Path to provider configuration JSON file
    #[clap(long = "config-json")]
    config_json: Option<PathBuf>,

    /// By default, the command will wait until the provider has been started.
    /// If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the provider to start.
    #[clap(long = "skip-wait")]
    skip_wait: bool,

    /// Timeout to await the provider start, defaults to 15000 milliseconds.
    #[clap(long = "timeout-ms", default_value_t = 15000)]
    timeout_ms: u64,
}

#[derive(Debug, Clone, Parser)]
pub(crate) struct StopActorCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,

    /// Id of host
    #[clap(name = "host-id", parse(try_from_str))]
    pub(crate) host_id: ServerId,

    /// Actor Id, e.g. the public key for the actor
    #[clap(name = "actor-id", parse(try_from_str))]
    pub(crate) actor_id: ModuleId,

    /// Number of actors to stop
    #[clap(long = "count", default_value = "1")]
    pub(crate) count: u16,

    /// By default, the command will wait until the actor has been stopped.
    /// If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the actor to stp[].
    #[clap(long = "skip-wait")]
    skip_wait: bool,

    /// Timeout to await the actor stop, defaults to 3000 milliseconds.
    #[clap(long = "timeout-ms", default_value_t = 3000)]
    timeout_ms: u64,
}

#[derive(Debug, Clone, Parser)]
pub(crate) struct StopProviderCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,

    /// Id of host
    #[clap(name = "host-id", parse(try_from_str))]
    host_id: ServerId,

    /// Provider Id, e.g. the public key for the provider
    #[clap(name = "provider-id", parse(try_from_str))]
    pub(crate) provider_id: ServiceId,

    /// Link name of provider
    #[clap(name = "link-name")]
    pub(crate) link_name: String,

    /// Capability contract Id of provider
    #[clap(name = "contract-id")]
    pub(crate) contract_id: String,

    /// By default, the command will wait until the provider has been stopped.
    /// If this flag is passed, the command will return immediately after acknowledgement from the host, without waiting for the provider to stop.
    #[clap(long = "skip-wait")]
    skip_wait: bool,

    /// Timeout to await the provider stop, defaults to 3000 milliseconds.
    #[clap(long = "timeout-ms", default_value_t = 3000)]
    timeout_ms: u64,
}

#[derive(Debug, Clone, Parser)]
pub(crate) struct StopHostCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,

    /// Id of host
    #[clap(name = "host-id", parse(try_from_str))]
    host_id: ServerId,

    /// The timeout in ms for how much time to give the host for graceful shutdown
    #[clap(
        short = 'h',
        long = "host-timeout",
        default_value_t = default_timeout_ms()
    )]
    host_shutdown_timeout: u64,
}

#[derive(Debug, Clone, Parser)]
pub(crate) struct UpdateActorCommand {
    #[clap(flatten)]
    opts: ConnectionOpts,

    /// Id of host
    #[clap(name = "host-id", parse(try_from_str))]
    pub(crate) host_id: ServerId,

    /// Actor Id, e.g. the public key for the actor
    #[clap(name = "actor-id", parse(try_from_str))]
    pub(crate) actor_id: ModuleId,

    /// Actor reference, e.g. the OCI URL for the actor.
    #[clap(name = "new-actor-ref")]
    pub(crate) new_actor_ref: String,
}

pub(crate) async fn handle_command(
    command: CtlCliCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    use CtlCliCommand::*;
    let sp: Spinner = Spinner::new(&output_kind);
    let out: CommandOutput = match command {
        Apply(cmd) => {
            sp.update_spinner_message(" Applying manifest ...".to_string());
            let results = apply_manifest(cmd).await?;
            apply_manifest_output(results)
        }
        Get(GetCommand::Hosts(cmd)) => {
            sp.update_spinner_message(" Retrieving Hosts ...".to_string());
            let hosts = get_hosts(cmd).await?;
            get_hosts_output(hosts)
        }
        Get(GetCommand::HostInventory(cmd)) => {
            sp.update_spinner_message(format!(
                " Retrieving inventory for host {} ...",
                cmd.host_id
            ));
            let inv = get_host_inventory(cmd).await?;
            get_host_inventory_output(inv)
        }
        Get(GetCommand::Claims(cmd)) => {
            sp.update_spinner_message(" Retrieving claims ... ".to_string());
            let claims = get_claims(cmd).await?;
            get_claims_output(claims)
        }
        Link(LinkCommand::Del(cmd)) => {
            let link_name = &cmd
                .link_name
                .clone()
                .unwrap_or_else(|| "default".to_string());

            validate_contract_id(&cmd.contract_id)?;

            sp.update_spinner_message(format!(
                "Deleting link for {} on {} ({}) ... ",
                cmd.actor_id, cmd.contract_id, link_name,
            ));

            let failure = link_del(cmd.clone())
                .await
                .map_or_else(|e| Some(format!("{}", e)), |_| None);
            link_del_output(&cmd.actor_id, &cmd.contract_id, link_name, failure)?
        }
        Link(LinkCommand::Put(cmd)) => {
            validate_contract_id(&cmd.contract_id)?;

            sp.update_spinner_message(format!(
                "Defining link between {} and {} ... ",
                cmd.actor_id, cmd.provider_id
            ));

            let failure = link_put(cmd.clone())
                .await
                .map_or_else(|e| Some(format!("{}", e)), |_| None);
            link_put_output(&cmd.actor_id, &cmd.provider_id, failure)?
        }
        Link(LinkCommand::Query(cmd)) => {
            sp.update_spinner_message("Querying Links ... ".to_string());
            let result = link_query(cmd.clone()).await?;
            link_query_output(result)
        }
        Start(StartCommand::Actor(cmd)) => {
            let actor_ref = &cmd.actor_ref.to_string();

            sp.update_spinner_message(format!(" Starting actor {} ... ", actor_ref));

            start_actor(cmd).await?
        }
        Start(StartCommand::Provider(cmd)) => {
            let provider_ref = &cmd.provider_ref.to_string();

            sp.update_spinner_message(format!(" Starting provider {} ... ", provider_ref));

            start_provider(cmd).await?
        }
        Stop(StopCommand::Actor(cmd)) => {
            sp.update_spinner_message(format!(" Stopping actor {} ... ", cmd.actor_id));

            stop_actor(cmd.clone()).await?
        }
        Stop(StopCommand::Provider(cmd)) => {
            sp.update_spinner_message(format!(" Stopping provider {} ... ", cmd.provider_id));

            stop_provider(cmd.clone()).await?
        }
        Stop(StopCommand::Host(cmd)) => {
            sp.update_spinner_message(format!(" Stopping host {} ... ", cmd.host_id));

            let ack = stop_host(cmd.clone()).await?;
            if !ack.accepted {
                bail!("Operation failed: {}", ack.error);
            }

            CommandOutput::from_key_and_text(
                "result",
                format!("Host {} acknowledged stop request", cmd.host_id),
            )
        }
        Update(UpdateCommand::Actor(cmd)) => {
            sp.update_spinner_message(format!(
                " Updating Actor {} to {} ... ",
                cmd.actor_id, cmd.new_actor_ref
            ));

            let ack = update_actor(cmd.clone()).await?;
            if !ack.accepted {
                bail!("Operation failed: {}", ack.error);
            }

            CommandOutput::from_key_and_text(
                "result",
                format!("Actor {} updated to {}", cmd.actor_id, cmd.new_actor_ref),
            )
        }
        Scale(ScaleCommand::Actor(cmd)) => {
            sp.update_spinner_message(format!(
                " Scaling Actor {} to {} instances ... ",
                cmd.actor_id, cmd.count
            ));
            scale_actor(cmd.clone()).await?
        }
    };

    sp.finish_and_clear();

    Ok(out)
}

pub(crate) async fn get_hosts(cmd: GetHostsCommand) -> Result<Vec<Host>> {
    let client = ctl_client_from_opts(cmd.opts, None).await?;
    client.get_hosts().await.map_err(convert_error)
}

pub(crate) async fn get_host_inventory(cmd: GetHostInventoryCommand) -> Result<HostInventory> {
    let client = ctl_client_from_opts(cmd.opts, None).await?;
    client
        .get_host_inventory(&cmd.host_id.to_string())
        .await
        .map_err(convert_error)
}

pub(crate) async fn get_claims(cmd: GetClaimsCommand) -> Result<GetClaimsResponse> {
    let client = ctl_client_from_opts(cmd.opts, None).await?;
    client.get_claims().await.map_err(convert_error)
}

pub(crate) async fn link_del(cmd: LinkDelCommand) -> Result<CtlOperationAck> {
    let client = ctl_client_from_opts(cmd.opts, None).await?;
    client
        .remove_link(
            &cmd.actor_id.to_string(),
            &cmd.contract_id,
            &cmd.link_name.unwrap_or_else(|| "default".to_string()),
        )
        .await
        .map_err(convert_error)
}

pub(crate) async fn link_put(cmd: LinkPutCommand) -> Result<CtlOperationAck> {
    let client = ctl_client_from_opts(cmd.opts, None).await?;
    client
        .advertise_link(
            &cmd.actor_id.to_string(),
            &cmd.provider_id.to_string(),
            &cmd.contract_id,
            &cmd.link_name.unwrap_or_else(|| "default".to_string()),
            labels_vec_to_hashmap(cmd.values)?,
        )
        .await
        .map_err(convert_error)
}

pub(crate) async fn link_query(cmd: LinkQueryCommand) -> Result<LinkDefinitionList> {
    let client = ctl_client_from_opts(cmd.opts, None).await?;
    client.query_links().await.map_err(convert_error)
}

pub(crate) async fn start_actor(mut cmd: StartActorCommand) -> Result<CommandOutput> {
    // If timeout isn't supplied, override with a longer timeout for starting actor
    if cmd.opts.ack_timeout_ms == DEFAULT_NATS_TIMEOUT_MS {
        cmd.opts.ack_timeout_ms = DEFAULT_START_PROVIDER_TIMEOUT_MS;
    }
    let client = ctl_client_from_opts(cmd.opts, Some(cmd.auction_timeout_ms)).await?;

    let host = match cmd.host_id {
        Some(host) => host,
        None => {
            let suitable_hosts = client
                .perform_actor_auction(
                    &cmd.actor_ref,
                    labels_vec_to_hashmap(cmd.constraints.unwrap_or_default())?,
                )
                .await
                .map_err(convert_error)?;
            if suitable_hosts.is_empty() {
                bail!("No suitable hosts found for actor {}", cmd.actor_ref);
            } else {
                suitable_hosts[0].host_id.parse()?
            }
        }
    };

    let receiver = client.events_receiver().await.map_err(convert_error)?;

    let ack = client
        .start_actor(&host.to_string(), &cmd.actor_ref, cmd.count, None)
        .await
        .map_err(convert_error)?;

    if !ack.accepted {
        bail!("Operation failed: {}", ack.error);
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
        &receiver,
        Duration::from_millis(cmd.timeout_ms),
        host.to_string(),
        cmd.actor_ref.clone(),
    )?;

    match event {
        FindEventOutcome::Success(_) => Ok(CommandOutput::from_key_and_text(
            "result",
            format!("Actor {} started on host {}", cmd.actor_ref, host),
        )),
        FindEventOutcome::Failure(err) => bail!("{}", err),
    }
}

pub(crate) async fn start_provider(mut cmd: StartProviderCommand) -> Result<CommandOutput> {
    // If timeout isn't supplied, override with a longer timeout for starting provider
    if cmd.opts.ack_timeout_ms == DEFAULT_NATS_TIMEOUT_MS {
        cmd.opts.ack_timeout_ms = DEFAULT_START_PROVIDER_TIMEOUT_MS;
    }
    // OCI downloads and response
    let client = ctl_client_from_opts(cmd.opts, Some(cmd.auction_timeout_ms)).await?;

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
                .map_err(convert_error)?;
            if suitable_hosts.is_empty() {
                bail!("No suitable hosts found for provider {}", cmd.provider_ref);
            } else {
                suitable_hosts[0].host_id.parse()?
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

    let receiver = client.events_receiver().await.map_err(convert_error)?;

    let ack = client
        .start_provider(
            &host.to_string(),
            &cmd.provider_ref,
            Some(cmd.link_name),
            None,
            config_json,
        )
        .await
        .map_err(convert_error)?;

    if !ack.accepted {
        bail!("Operation failed: {}", ack.error);
    }

    if cmd.skip_wait {
        return Ok(CommandOutput::from_key_and_text(
            "result",
            format!("Start provider request received: {}", &cmd.provider_ref),
        ));
    }

    let event = wait_for_provider_start_event(
        &receiver,
        Duration::from_millis(cmd.timeout_ms),
        host.to_string(),
        cmd.provider_ref.clone(),
    )?;

    match event {
        FindEventOutcome::Success(_) => Ok(CommandOutput::from_key_and_text(
            "result",
            format!("Provider {} started on host {}", cmd.provider_ref, host),
        )),
        FindEventOutcome::Failure(err) => {
            bail!("{}", err);
        }
    }
}

pub(crate) async fn scale_actor(cmd: ScaleActorCommand) -> Result<CommandOutput> {
    let client = ctl_client_from_opts(cmd.opts, None).await?;

    let annotations = labels_vec_to_hashmap(cmd.annotations)?;

    let ack = client
        .scale_actor(
            &cmd.host_id.to_string(),
            &cmd.actor_ref,
            &cmd.actor_id.to_string(),
            cmd.count,
            Some(annotations),
        )
        .await
        .map_err(convert_error)?;

    if !ack.accepted {
        bail!("Operation failed: {}", ack.error);
    }

    Ok(CommandOutput::from_key_and_text(
        "result",
        format!(
            "Request to scale actor {} to {} instances recieved",
            cmd.actor_id, cmd.count
        ),
    ))
}

pub(crate) async fn stop_provider(cmd: StopProviderCommand) -> Result<CommandOutput> {
    validate_contract_id(&cmd.contract_id)?;
    let client = ctl_client_from_opts(cmd.opts, None).await?;

    let receiver = client.events_receiver().await.map_err(convert_error)?;

    let ack = client
        .stop_provider(
            &cmd.host_id.to_string(),
            &cmd.provider_id.to_string(),
            &cmd.link_name,
            &cmd.contract_id,
            None,
        )
        .await
        .map_err(convert_error)?;

    if !ack.accepted {
        bail!("Operation failed: {}", ack.error);
    }
    if cmd.skip_wait {
        return Ok(CommandOutput::from_key_and_text(
            "result",
            format!("Provider {} stop request received", cmd.provider_id),
        ));
    }

    let event = wait_for_provider_stop_event(
        &receiver,
        Duration::from_millis(cmd.timeout_ms),
        cmd.host_id.to_string(),
        cmd.provider_id.to_string(),
    )?;

    match event {
        FindEventOutcome::Success(_) => Ok(CommandOutput::from_key_and_text(
            "result",
            format!("Provider {} stopped successfully", cmd.provider_id),
        )),
        FindEventOutcome::Failure(err) => bail!("{}", err),
    }
}

pub(crate) async fn stop_actor(cmd: StopActorCommand) -> Result<CommandOutput> {
    let client = ctl_client_from_opts(cmd.opts, None).await?;

    let receiver = client.events_receiver().await.map_err(convert_error)?;

    let ack = client
        .stop_actor(
            &cmd.host_id.to_string(),
            &cmd.actor_id.to_string(),
            cmd.count,
            None,
        )
        .await
        .map_err(convert_error)?;

    if !ack.accepted {
        bail!("Operation failed: {}", ack.error);
    }

    if cmd.skip_wait {
        return Ok(CommandOutput::from_key_and_text(
            "result",
            format!("Request to stop actor {} received", cmd.actor_id),
        ));
    }

    let event = wait_for_actor_stop_event(
        &receiver,
        Duration::from_millis(cmd.timeout_ms),
        cmd.host_id.to_string(),
        cmd.actor_id.to_string(),
    )?;

    match event {
        FindEventOutcome::Success(_) => Ok(CommandOutput::from_key_and_text(
            "result",
            format!("Actor {} stopped", cmd.actor_id),
        )),
        FindEventOutcome::Failure(err) => bail!("{}", err),
    }
}

pub(crate) async fn stop_host(cmd: StopHostCommand) -> Result<CtlOperationAck> {
    let client = ctl_client_from_opts(cmd.opts, None).await?;
    client
        .stop_host(&cmd.host_id.to_string(), Some(cmd.host_shutdown_timeout))
        .await
        .map_err(convert_error)
}

pub(crate) async fn update_actor(cmd: UpdateActorCommand) -> Result<CtlOperationAck> {
    let client = ctl_client_from_opts(cmd.opts, None).await?;
    client
        .update_actor(
            &cmd.host_id.to_string(),
            &cmd.actor_id.to_string(),
            &cmd.new_actor_ref,
            None,
        )
        .await
        .map_err(convert_error)
}

pub(crate) async fn apply_manifest(cmd: ApplyCommand) -> Result<Vec<String>> {
    let client = ctl_client_from_opts(cmd.opts, None).await?;
    let hm = match HostManifest::from_path(Path::new(&cmd.path), cmd.expand_env) {
        Ok(hm) => hm,
        Err(e) => bail!("Failed to load manifest: {}", e),
    };
    let mut results = vec![];
    results.extend_from_slice(&apply_manifest_actors(&cmd.host_key, &client, &hm).await?);
    results.extend_from_slice(&apply_manifest_providers(&cmd.host_key, &client, &hm).await?);
    results.extend_from_slice(&apply_manifest_linkdefs(&client, &hm).await?);
    Ok(results)
}

async fn apply_manifest_actors(
    host_id: &ServerId,
    client: &CtlClient,
    hm: &HostManifest,
) -> Result<Vec<String>> {
    let mut results = vec![];

    for actor in hm.actors.iter() {
        match client
            .start_actor(&host_id.to_string(), actor, ONE_ACTOR, None)
            .await
        {
            Ok(ack) => {
                if ack.accepted {
                    results.push(format!(
                        "Instruction to start actor {} acknowledged.",
                        actor
                    ));
                } else {
                    results.push(format!(
                        "Instruction to start actor {} not acked: {}",
                        actor, ack.error
                    ));
                }
            }
            Err(e) => results.push(format!("Failed to send start actor: {}", e)),
        }
    }

    Ok(results)
}

async fn apply_manifest_linkdefs(client: &CtlClient, hm: &HostManifest) -> Result<Vec<String>> {
    let mut results = vec![];

    for ld in hm.links.iter() {
        match client
            .advertise_link(
                &ld.actor,
                &ld.provider_id,
                &ld.contract_id,
                ld.link_name.as_ref().unwrap_or(&"default".to_string()),
                ld.values.clone().unwrap_or_default(),
            )
            .await
        {
            Ok(ack) => {
                if ack.accepted {
                    results.push(format!(
                        "Link def submission from {} to {} acknowledged.",
                        ld.actor, ld.provider_id
                    ));
                } else {
                    results.push(format!(
                        "Link def submission from {} to {} not acked: {}",
                        ld.actor, ld.provider_id, ack.error
                    ));
                }
            }
            Err(e) => results.push(format!("Failed to send link def: {}", e)),
        }
    }

    Ok(results)
}

async fn apply_manifest_providers(
    host_id: &ServerId,
    client: &CtlClient,
    hm: &HostManifest,
) -> Result<Vec<String>> {
    let mut results = vec![];

    for cap in hm.capabilities.iter() {
        match client
            .start_provider(
                &host_id.to_string(),
                &cap.image_ref,
                cap.link_name.clone(),
                None,
                None,
            )
            .await
        {
            Ok(ack) => {
                if ack.accepted {
                    results.push(format!(
                        "Instruction to start provider {} acknowledged.",
                        cap.image_ref
                    ));
                } else {
                    results.push(format!(
                        "Instruction to start provider {} not acked: {}",
                        cap.image_ref, ack.error
                    ));
                }
            }
            Err(e) => results.push(format!("Failed to send start capability message: {}", e)),
        }
    }

    Ok(results)
}

async fn ctl_client_from_opts(
    opts: ConnectionOpts,
    auction_timeout_ms: Option<u64>,
) -> Result<CtlClient> {
    // Attempt to load a context, falling back on the default if not supplied
    let ctx = if let Some(context) = opts.context {
        load_context(&context).ok()
    } else if let Ok(ctx_dir) = context_dir(None) {
        get_default_context(&ctx_dir).ok()
    } else {
        None
    };

    let lattice_prefix = opts.lattice_prefix.unwrap_or_else(|| {
        ctx.as_ref()
            .map(|c| c.ctl_lattice_prefix.clone())
            .unwrap_or_else(|| DEFAULT_LATTICE_PREFIX.to_string())
    });

    let ctl_host = opts.ctl_host.unwrap_or_else(|| {
        ctx.as_ref()
            .map(|c| c.ctl_host.clone())
            .unwrap_or_else(|| DEFAULT_NATS_HOST.to_string())
    });

    let ctl_port = opts.ctl_port.unwrap_or_else(|| {
        ctx.as_ref()
            .map(|c| c.ctl_port.to_string())
            .unwrap_or_else(|| DEFAULT_NATS_PORT.to_string())
    });

    let ctl_jwt = if opts.ctl_jwt.is_some() {
        opts.ctl_jwt
    } else {
        ctx.as_ref().map(|c| c.ctl_jwt.clone()).unwrap_or_default()
    };

    let ctl_seed = if opts.ctl_seed.is_some() {
        opts.ctl_seed
    } else {
        ctx.as_ref().map(|c| c.ctl_seed.clone()).unwrap_or_default()
    };

    let ctl_credsfile = if opts.ctl_credsfile.is_some() {
        opts.ctl_credsfile
    } else {
        ctx.as_ref()
            .map(|c| c.ctl_credsfile.clone())
            .unwrap_or_default()
    };
    let auction_timeout_ms = auction_timeout_ms.unwrap_or(DEFAULT_NATS_TIMEOUT_MS);

    let nc =
        crate::util::nats_client_from_opts(&ctl_host, &ctl_port, ctl_jwt, ctl_seed, ctl_credsfile)
            .await?;
    let ctl_client = CtlClient::new(
        nc,
        Some(lattice_prefix),
        Duration::from_millis(opts.ack_timeout_ms),
        Duration::from_millis(auction_timeout_ms),
    );

    Ok(ctl_client)
}

#[cfg(test)]
mod test {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cmd {
        #[clap(subcommand)]
        command: CtlCliCommand,
    }

    const CTL_HOST: &str = "127.0.0.1";
    const CTL_PORT: &str = "4222";
    const LATTICE_PREFIX: &str = "default";

    const ACTOR_ID: &str = "MDPDJEYIAK6MACO67PRFGOSSLODBISK4SCEYDY3HEOY4P5CVJN6UCWUK";
    const PROVIDER_ID: &str = "VBKTSBG2WKP6RJWLQ5O7RDVIIB4LMW6U5R67A7QMIDBZDGZWYTUE3TSI";
    const HOST_ID: &str = "NCE7YHGI42RWEKBRDJZWXBEJJCFNE5YIWYMSTLGHQBEGFY55BKJ3EG3G";

    #[test]
    /// Enumerates multiple options of the `ctl` command to ensure API doesn't
    /// change between versions. This test will fail if any subcommand of `wash ctl`
    /// changes syntax, ordering of required elements, or flags.
    fn test_ctl_comprehensive() -> Result<()> {
        let start_actor_all: Cmd = Parser::try_parse_from(&[
            "ctl",
            "start",
            "actor",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
            "--auction-timeout-ms",
            "2002",
            "--constraint",
            "arch=x86_64",
            "--host-id",
            HOST_ID,
            "wasmcloud.azurecr.io/actor:v1",
        ])?;
        match start_actor_all.command {
            CtlCliCommand::Start(StartCommand::Actor(super::StartActorCommand {
                opts,
                host_id,
                actor_ref,
                constraints,
                auction_timeout_ms,
                timeout_ms,
                ..
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(timeout_ms, 2001);
                assert_eq!(auction_timeout_ms, 2002);
                assert_eq!(host_id.unwrap(), HOST_ID.parse()?);
                assert_eq!(actor_ref, "wasmcloud.azurecr.io/actor:v1".to_string());
                assert_eq!(constraints.unwrap(), vec!["arch=x86_64".to_string()]);
            }
            cmd => panic!("ctl start actor constructed incorrect command {:?}", cmd),
        }
        let start_provider_all: Cmd = Parser::try_parse_from(&[
            "ctl",
            "start",
            "provider",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--ack-timeout-ms",
            "2001",
            "--auction-timeout-ms",
            "2002",
            "--constraint",
            "arch=x86_64",
            "--host-id",
            HOST_ID,
            "--link-name",
            "default",
            "--skip-wait",
            "wasmcloud.azurecr.io/provider:v1",
        ])?;
        match start_provider_all.command {
            CtlCliCommand::Start(StartCommand::Provider(super::StartProviderCommand {
                opts,
                host_id,
                provider_ref,
                link_name,
                constraints,
                auction_timeout_ms,
                config_json,
                skip_wait,
                timeout_ms,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.ack_timeout_ms, 2001);
                assert_eq!(config_json, None);
                assert_eq!(auction_timeout_ms, 2002);
                assert_eq!(link_name, "default".to_string());
                assert_eq!(constraints.unwrap(), vec!["arch=x86_64".to_string()]);
                assert_eq!(host_id.unwrap(), HOST_ID.parse()?);
                assert_eq!(provider_ref, "wasmcloud.azurecr.io/provider:v1".to_string());
                assert!(skip_wait);
                assert_eq!(timeout_ms, 15000);
            }
            cmd => panic!("ctl start provider constructed incorrect command {:?}", cmd),
        }
        let stop_actor_all: Cmd = Parser::try_parse_from(&[
            "ctl",
            "stop",
            "actor",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--ack-timeout-ms",
            "2001",
            "--count",
            "2",
            HOST_ID,
            ACTOR_ID,
        ])?;
        match stop_actor_all.command {
            CtlCliCommand::Stop(StopCommand::Actor(super::StopActorCommand {
                opts,
                host_id,
                actor_id,
                count,
                skip_wait,
                timeout_ms,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.ack_timeout_ms, 2001);
                assert_eq!(host_id, HOST_ID.parse()?);
                assert_eq!(actor_id, ACTOR_ID.parse()?);
                assert_eq!(count, 2);
                assert!(!skip_wait);
                assert_eq!(timeout_ms, 3000);
            }
            cmd => panic!("ctl stop actor constructed incorrect command {:?}", cmd),
        }
        let stop_provider_all: Cmd = Parser::try_parse_from(&[
            "ctl",
            "stop",
            "provider",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--ack-timeout-ms",
            "2001",
            HOST_ID,
            PROVIDER_ID,
            "default",
            "wasmcloud:provider",
        ])?;
        match stop_provider_all.command {
            CtlCliCommand::Stop(StopCommand::Provider(super::StopProviderCommand {
                opts,
                host_id,
                provider_id,
                link_name,
                contract_id,
                skip_wait,
                timeout_ms,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.ack_timeout_ms, 2001);
                assert_eq!(host_id, HOST_ID.parse()?);
                assert_eq!(provider_id, PROVIDER_ID.parse()?);
                assert_eq!(link_name, "default".to_string());
                assert_eq!(contract_id, "wasmcloud:provider".to_string());
                assert!(!skip_wait);
                assert_eq!(timeout_ms, 3000);
            }
            cmd => panic!("ctl stop actor constructed incorrect command {:?}", cmd),
        }
        let get_hosts_all: Cmd = Parser::try_parse_from(&[
            "ctl",
            "get",
            "hosts",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--ack-timeout-ms",
            "2001",
        ])?;
        match get_hosts_all.command {
            CtlCliCommand::Get(GetCommand::Hosts(GetHostsCommand { opts })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.ack_timeout_ms, 2001);
            }
            cmd => panic!("ctl get hosts constructed incorrect command {:?}", cmd),
        }
        let get_host_inventory_all: Cmd = Parser::try_parse_from(&[
            "ctl",
            "get",
            "inventory",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--ack-timeout-ms",
            "2001",
            HOST_ID,
        ])?;
        match get_host_inventory_all.command {
            CtlCliCommand::Get(GetCommand::HostInventory(GetHostInventoryCommand {
                opts,
                host_id,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.ack_timeout_ms, 2001);
                assert_eq!(host_id, HOST_ID.parse()?);
            }
            cmd => panic!("ctl get inventory constructed incorrect command {:?}", cmd),
        }
        let get_claims_all: Cmd = Parser::try_parse_from(&[
            "ctl",
            "get",
            "claims",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--ack-timeout-ms",
            "2001",
        ])?;
        match get_claims_all.command {
            CtlCliCommand::Get(GetCommand::Claims(GetClaimsCommand { opts })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.ack_timeout_ms, 2001);
            }
            cmd => panic!("ctl get claims constructed incorrect command {:?}", cmd),
        }
        let link_all: Cmd = Parser::try_parse_from(&[
            "ctl",
            "link",
            "put",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--ack-timeout-ms",
            "2001",
            "--link-name",
            "default",
            ACTOR_ID,
            PROVIDER_ID,
            "wasmcloud:provider",
            "THING=foo",
        ])?;
        match link_all.command {
            CtlCliCommand::Link(LinkCommand::Put(LinkPutCommand {
                opts,
                actor_id,
                provider_id,
                contract_id,
                link_name,
                values,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.ack_timeout_ms, 2001);
                assert_eq!(actor_id, ACTOR_ID.parse()?);
                assert_eq!(provider_id, PROVIDER_ID.parse()?);
                assert_eq!(contract_id, "wasmcloud:provider".to_string());
                assert_eq!(link_name.unwrap(), "default".to_string());
                assert_eq!(values, vec!["THING=foo".to_string()]);
            }
            cmd => panic!("ctl link put constructed incorrect command {:?}", cmd),
        }
        let update_all: Cmd = Parser::try_parse_from(&[
            "ctl",
            "update",
            "actor",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--ack-timeout-ms",
            "2001",
            HOST_ID,
            ACTOR_ID,
            "wasmcloud.azurecr.io/actor:v2",
        ])?;
        match update_all.command {
            CtlCliCommand::Update(UpdateCommand::Actor(super::UpdateActorCommand {
                opts,
                host_id,
                actor_id,
                new_actor_ref,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.ack_timeout_ms, 2001);
                assert_eq!(host_id, HOST_ID.parse()?);
                assert_eq!(actor_id, ACTOR_ID.parse()?);
                assert_eq!(new_actor_ref, "wasmcloud.azurecr.io/actor:v2".to_string());
            }
            cmd => panic!("ctl get claims constructed incorrect command {:?}", cmd),
        }

        let scale_actor_all: Cmd = Parser::try_parse_from(&[
            "ctl",
            "scale",
            "actor",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--ack-timeout-ms",
            "2001",
            HOST_ID,
            ACTOR_ID,
            "wasmcloud.azurecr.io/actor:v2",
            "--count",
            "1",
            "--annotations",
            "foo=bar",
        ])?;

        match scale_actor_all.command {
            CtlCliCommand::Scale(ScaleCommand::Actor(super::ScaleActorCommand {
                opts,
                host_id,
                actor_id,
                actor_ref,
                count,
                annotations,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.ack_timeout_ms, 2001);
                assert_eq!(host_id, HOST_ID.parse()?);
                assert_eq!(actor_id, ACTOR_ID.parse()?);
                assert_eq!(actor_ref, "wasmcloud.azurecr.io/actor:v2".to_string());
                assert_eq!(count, 1);
                assert_eq!(annotations, vec!["foo=bar".to_string()]);
            }
            cmd => panic!("ctl scale actor constructed incorrect command {:?}", cmd),
        }

        Ok(())
    }
}
