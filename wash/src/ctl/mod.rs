extern crate wasmcloud_control_interface;
use crate::util::{convert_error, labels_vec_to_hashmap, Output, OutputKind, Result};
use spinners::{Spinner, Spinners};
use std::time::Duration;
use structopt::StructOpt;
use wasmcloud_control_interface::{
    Client as CtlClient, CtlOperationAck, GetClaimsResponse, Host, HostInventory,
};
mod output;
pub(crate) use output::*;
#[derive(Debug, Clone, StructOpt)]
pub(crate) struct CtlCli {
    #[structopt(flatten)]
    command: CtlCliCommand,
}

impl CtlCli {
    pub(crate) fn command(self) -> CtlCliCommand {
        self.command
    }
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct ConnectionOpts {
    /// CTL Host for connection, defaults to 0.0.0.0 for local nats
    #[structopt(
        short = "r",
        long = "ctl-host",
        default_value = "0.0.0.0",
        env = "WASH_CTL_HOST"
    )]
    ctl_host: String,

    /// CTL Port for connections, defaults to 4222 for local nats
    #[structopt(
        short = "p",
        long = "ctl-port",
        default_value = "4222",
        env = "WASH_CTL_PORT"
    )]
    ctl_port: String,

    /// JWT file for CTL authentication. Must be supplied with ctl_seed.
    #[structopt(long = "ctl-jwt", env = "WASH_CTL_JWT", hide_env_values = true)]
    ctl_jwt: Option<String>,

    /// Seed file or literal for CTL authentication. Must be supplied with ctl_jwt.
    #[structopt(long = "ctl-seed", env = "WASH_CTL_SEED", hide_env_values = true)]
    ctl_seed: Option<String>,

    /// Credsfile for CTL authentication. Combines ctl_seed and ctl_jwt.
    /// See https://docs.nats.io/developing-with-nats/security/creds for details.
    #[structopt(long = "ctl-credsfile", env = "WASH_CTL_CREDS", hide_env_values = true)]
    ctl_credsfile: Option<String>,

    /// Namespace prefix for wasmcloud control interface
    #[structopt(
        short = "n",
        long = "ns-prefix",
        default_value = "default",
        env = "WASH_CTL_NSPREFIX"
    )]
    ns_prefix: String,

    /// Timeout length to await a control interface response
    #[structopt(long = "timeout", default_value = "1")]
    timeout: u64,
}

impl Default for ConnectionOpts {
    fn default() -> Self {
        ConnectionOpts {
            ctl_host: "0.0.0.0".to_string(),
            ctl_port: "4222".to_string(),
            ctl_jwt: None,
            ctl_seed: None,
            ctl_credsfile: None,
            ns_prefix: "default".to_string(),
            timeout: 1,
        }
    }
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) enum CtlCliCommand {
    /// Retrieves information about the lattice
    #[structopt(name = "get")]
    Get(GetCommand),

    /// Link an actor and a provider
    #[structopt(name = "link")]
    Link(LinkCommand),

    /// Start an actor or a provider
    #[structopt(name = "start")]
    Start(StartCommand),

    /// Stop an actor or a provider
    #[structopt(name = "stop")]
    Stop(StopCommand),

    /// Update an actor running in a host to a new actor
    #[structopt(name = "update")]
    Update(UpdateCommand),
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) enum GetCommand {
    /// Query lattice for running hosts
    #[structopt(name = "hosts")]
    Hosts(GetHostsCommand),

    /// Query a single host for its inventory of labels, actors and providers
    #[structopt(name = "inventory")]
    HostInventory(GetHostInventoryCommand),

    /// Query lattice for its claims cache
    #[structopt(name = "claims")]
    Claims(GetClaimsCommand),
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct LinkCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(flatten)]
    pub(crate) output: Output,

    /// Public key ID of actor
    #[structopt(name = "actor-id")]
    pub(crate) actor_id: String,

    /// Public key ID of provider
    #[structopt(name = "provider-id")]
    pub(crate) provider_id: String,

    /// Capability contract ID between actor and provider
    #[structopt(name = "contract-id")]
    pub(crate) contract_id: String,

    /// Link name, defaults to "default"
    #[structopt(short = "l", long = "link-name")]
    pub(crate) link_name: Option<String>,

    /// Environment values to provide alongside link
    #[structopt(name = "values")]
    pub(crate) values: Vec<String>,
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) enum StartCommand {
    /// Launch an actor in a host
    #[structopt(name = "actor")]
    Actor(StartActorCommand),

    /// Launch a provider in a host
    #[structopt(name = "provider")]
    Provider(StartProviderCommand),
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) enum StopCommand {
    /// Stop an actor running in a host
    #[structopt(name = "actor")]
    Actor(StopActorCommand),

    /// Stop a provider running in a host
    #[structopt(name = "provider")]
    Provider(StopProviderCommand),
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) enum UpdateCommand {
    /// Update an actor running in a host
    #[structopt(name = "actor")]
    Actor(UpdateActorCommand),
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct GetHostsCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(flatten)]
    pub(crate) output: Output,
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct GetHostInventoryCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(flatten)]
    pub(crate) output: Output,

    /// Id of host
    #[structopt(name = "host-id")]
    pub(crate) host_id: String,
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct GetClaimsCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(flatten)]
    pub(crate) output: Output,
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct StartActorCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(flatten)]
    pub(crate) output: Output,

    /// Id of host, if omitted the actor will be auctioned in the lattice to find a suitable host
    #[structopt(short = "h", long = "host-id", name = "host-id")]
    pub(crate) host_id: Option<String>,

    /// Actor reference, e.g. the OCI URL for the actor. This can also be a signed local wasm file when using the REPL host
    #[structopt(name = "actor-ref")]
    pub(crate) actor_ref: String,

    /// Constraints for actor auction in the form of "label=value". If host-id is supplied, this list is ignored
    #[structopt(short = "c", long = "constraint", name = "constraints")]
    constraints: Option<Vec<String>>,
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct StartProviderCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(flatten)]
    pub(crate) output: Output,

    /// Id of host, if omitted the provider will be auctioned in the lattice to find a suitable host
    #[structopt(short = "h", long = "host-id", name = "host-id")]
    host_id: Option<String>,

    /// Provider reference, e.g. the OCI URL for the provider
    #[structopt(name = "provider-ref")]
    pub(crate) provider_ref: String,

    /// Link name of provider
    #[structopt(short = "l", long = "link-name", default_value = "default")]
    pub(crate) link_name: String,

    /// Constraints for provider auction in the form of "label=value". If host-id is supplied, this list is ignored
    #[structopt(short = "c", long = "constraint", name = "constraints")]
    constraints: Option<Vec<String>>,
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct StopActorCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(flatten)]
    pub(crate) output: Output,

    /// Id of host
    #[structopt(name = "host-id")]
    pub(crate) host_id: String,

    /// Actor Id, e.g. the public key for the actor
    #[structopt(name = "actor-id")]
    pub(crate) actor_id: String,

    /// Number of actors to stop
    #[structopt(long = "count", default_value = "1")]
    pub(crate) count: u16,
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct StopProviderCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(flatten)]
    pub(crate) output: Output,

    /// Id of host
    #[structopt(name = "host-id")]
    host_id: String,

    /// Provider Id, e.g. the public key for the provider
    #[structopt(name = "provider-id")]
    pub(crate) provider_id: String,

    /// Link name of provider
    #[structopt(name = "link-name")]
    pub(crate) link_name: String,

    /// Capability contract Id of provider
    #[structopt(name = "contract-id")]
    pub(crate) contract_id: String,
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct UpdateActorCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(flatten)]
    pub(crate) output: Output,

    /// Id of host
    #[structopt(name = "host-id")]
    pub(crate) host_id: String,

    /// Actor Id, e.g. the public key for the actor
    #[structopt(name = "actor-id")]
    pub(crate) actor_id: String,

    /// Actor reference, e.g. the OCI URL for the actor. This can also be a signed local wasm file when using the REPL host
    #[structopt(name = "new-actor-ref")]
    pub(crate) new_actor_ref: String,
}

pub(crate) async fn handle_command(command: CtlCliCommand) -> Result<String> {
    use CtlCliCommand::*;
    let mut sp: Option<Spinner> = None;
    let out = match command {
        Get(GetCommand::Hosts(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(sp, " Retrieving Hosts ...".to_string(), &output);
            let hosts = get_hosts(cmd).await?;
            get_hosts_output(hosts, &output.kind)
        }
        Get(GetCommand::HostInventory(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(
                sp,
                format!(" Retrieving inventory for host {} ...", cmd.host_id),
                &output,
            );
            let inv = get_host_inventory(cmd).await?;
            get_host_inventory_output(inv, &output.kind)
        }
        Get(GetCommand::Claims(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(sp, " Retrieving claims ... ".to_string(), &output);
            let claims = get_claims(cmd).await?;
            get_claims_output(claims, &output.kind)
        }
        Link(cmd) => {
            sp = update_spinner_message(
                sp,
                format!(
                    " Advertising link between {} and {} ... ",
                    cmd.actor_id, cmd.provider_id
                ),
                &cmd.output,
            );
            let failure = advertise_link(cmd.clone())
                .await
                .map_or_else(|e| Some(format!("{}", e)), |_| None);
            link_output(&cmd.actor_id, &cmd.provider_id, failure, &cmd.output.kind)
        }
        Start(StartCommand::Actor(cmd)) => {
            let output = cmd.output;
            let actor_ref = &cmd.actor_ref.to_string();
            sp = update_spinner_message(sp, format!(" Starting actor {} ... ", actor_ref), &output);
            let ack = start_actor(cmd).await?;
            ctl_operation_output(
                ack.accepted,
                &format!("Actor {} started successfully", actor_ref),
                &ack.error,
                &output.kind,
            )
        }
        Start(StartCommand::Provider(cmd)) => {
            let output = cmd.output;
            let provider_ref = &cmd.provider_ref.to_string();
            sp = update_spinner_message(
                sp,
                format!(" Starting provider {} ... ", provider_ref),
                &output,
            );
            let ack = start_provider(cmd).await?;
            ctl_operation_output(
                ack.accepted,
                &format!("Provider {} started successfully", provider_ref),
                &ack.error,
                &output.kind,
            )
        }
        Stop(StopCommand::Actor(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(
                sp,
                format!(" Stopping actor {} ... ", cmd.actor_id),
                &output,
            );
            let ack = stop_actor(cmd.clone()).await?;
            ctl_operation_output(
                ack.accepted,
                &format!("Actor {} stopped successfully", cmd.actor_id),
                &ack.error,
                &output.kind,
            )
        }
        Stop(StopCommand::Provider(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(
                sp,
                format!(" Stopping provider {} ... ", cmd.provider_id),
                &output,
            );
            let ack = stop_provider(cmd.clone()).await?;
            ctl_operation_output(
                ack.accepted,
                &format!("Provider {} stopped successfully", cmd.provider_id),
                &ack.error,
                &output.kind,
            )
        }
        Update(UpdateCommand::Actor(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(
                sp,
                format!(
                    " Updating Actor {} to {} ... ",
                    cmd.actor_id, cmd.new_actor_ref
                ),
                &output,
            );
            let ack = update_actor(cmd.clone()).await?;
            ctl_operation_output(
                ack.accepted,
                &format!("Actor {} updated to {}", cmd.actor_id, cmd.new_actor_ref),
                &ack.error,
                &output.kind,
            )
        }
    };

    if sp.is_some() {
        sp.unwrap().stop()
    }

    Ok(out)
}

pub(crate) async fn get_hosts(cmd: GetHostsCommand) -> Result<Vec<Host>> {
    let timeout = Duration::from_secs(cmd.opts.timeout);
    let client = ctl_client_from_opts(cmd.opts).await?;
    client.get_hosts(timeout).await.map_err(convert_error)
}

pub(crate) async fn get_host_inventory(cmd: GetHostInventoryCommand) -> Result<HostInventory> {
    let client = ctl_client_from_opts(cmd.opts).await?;
    client
        .get_host_inventory(&cmd.host_id)
        .await
        .map_err(convert_error)
}

pub(crate) async fn get_claims(cmd: GetClaimsCommand) -> Result<GetClaimsResponse> {
    let client = ctl_client_from_opts(cmd.opts).await?;
    client.get_claims().await.map_err(convert_error)
}

pub(crate) async fn advertise_link(cmd: LinkCommand) -> Result<CtlOperationAck> {
    let client = ctl_client_from_opts(cmd.opts).await?;
    client
        .advertise_link(
            &cmd.actor_id,
            &cmd.provider_id,
            &cmd.contract_id,
            &cmd.link_name.unwrap_or_else(|| "default".to_string()),
            labels_vec_to_hashmap(cmd.values)?,
        )
        .await
        .map_err(convert_error)
}

pub(crate) async fn start_actor(cmd: StartActorCommand) -> Result<CtlOperationAck> {
    let timeout = Duration::from_secs(cmd.opts.timeout);
    let client = ctl_client_from_opts(cmd.opts).await?;

    let host = match cmd.host_id {
        Some(host) => host,
        None => {
            let suitable_hosts = client
                .perform_actor_auction(
                    &cmd.actor_ref,
                    labels_vec_to_hashmap(cmd.constraints.unwrap_or_default())?,
                    timeout,
                )
                .await
                .map_err(convert_error)?;
            if suitable_hosts.is_empty() {
                return Err(format!("No suitable hosts found for actor {}", cmd.actor_ref).into());
            } else {
                suitable_hosts[0].host_id.to_string()
            }
        }
    };

    client
        .start_actor(&host, &cmd.actor_ref)
        .await
        .map_err(convert_error)
}

pub(crate) async fn start_provider(cmd: StartProviderCommand) -> Result<CtlOperationAck> {
    let timeout = Duration::from_secs(cmd.opts.timeout);
    let client = ctl_client_from_opts(cmd.opts).await?;

    let host = match cmd.host_id {
        Some(host) => host,
        None => {
            let suitable_hosts = client
                .perform_provider_auction(
                    &cmd.provider_ref,
                    &cmd.link_name,
                    labels_vec_to_hashmap(cmd.constraints.unwrap_or_default())?,
                    timeout,
                )
                .await
                .map_err(convert_error)?;
            if suitable_hosts.is_empty() {
                return Err(
                    format!("No suitable hosts found for provider {}", cmd.provider_ref).into(),
                );
            } else {
                suitable_hosts[0].host_id.to_string()
            }
        }
    };

    client
        .start_provider(&host, &cmd.provider_ref, Some(cmd.link_name))
        .await
        .map_err(convert_error)
}

pub(crate) async fn stop_provider(cmd: StopProviderCommand) -> Result<CtlOperationAck> {
    let client = ctl_client_from_opts(cmd.opts).await?;
    client
        .stop_provider(
            &cmd.host_id,
            &cmd.provider_id,
            &cmd.link_name,
            &cmd.contract_id,
        )
        .await
        .map_err(convert_error)
}

pub(crate) async fn stop_actor(cmd: StopActorCommand) -> Result<CtlOperationAck> {
    let client = ctl_client_from_opts(cmd.opts).await?;
    client
        .stop_actor(&cmd.host_id, &cmd.actor_id, cmd.count)
        .await
        .map_err(convert_error)
}

pub(crate) async fn update_actor(cmd: UpdateActorCommand) -> Result<CtlOperationAck> {
    let client = ctl_client_from_opts(cmd.opts).await?;
    client
        .update_actor(&cmd.host_id, &cmd.actor_id, &cmd.new_actor_ref)
        .await
        .map_err(convert_error)
}

async fn ctl_client_from_opts(opts: ConnectionOpts) -> Result<CtlClient> {
    let timeout = Duration::from_secs(opts.timeout);
    let nc = crate::util::nats_client_from_opts(
        &opts.ctl_host,
        &opts.ctl_port,
        opts.ctl_jwt,
        opts.ctl_seed,
        opts.ctl_credsfile,
    )
    .await?;
    let ctl_client = CtlClient::new(nc, Some(opts.ns_prefix.clone()), timeout);

    Ok(ctl_client)
}

/// Handles updating the spinner for text output
/// JSON output will be corrupted with a spinner
fn update_spinner_message(
    spinner: Option<Spinner>,
    msg: String,
    output: &Output,
) -> Option<Spinner> {
    if let Some(sp) = spinner {
        sp.message(msg);
        Some(sp)
    } else if matches!(output.kind, OutputKind::Text) {
        Some(Spinner::new(Spinners::Dots12, msg))
    } else {
        None
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const CTL_HOST: &str = "0.0.0.0";
    const CTL_PORT: &str = "4222";
    const NS_PREFIX: &str = "default";

    const ACTOR_ID: &str = "MDPDJEYIAK6MACO67PRFGOSSLODBISK4SCEYDY3HEOY4P5CVJN6UCWUK";
    const PROVIDER_ID: &str = "VBKTSBG2WKP6RJWLQ5O7RDVIIB4LMW6U5R67A7QMIDBZDGZWYTUE3TSI";
    const HOST_ID: &str = "NCE7YHGI42RWEKBRDJZWXBEJJCFNE5YIWYMSTLGHQBEGFY55BKJ3EG3G";

    #[test]
    /// Enumerates multiple options of the `ctl` command to ensure API doesn't
    /// change between versions. This test will fail if any subcommand of `wash ctl`
    /// changes syntax, ordering of required elements, or flags.
    fn test_ctl_comprehensive() -> Result<()> {
        let start_actor_all = CtlCli::from_iter_safe(&[
            "ctl",
            "start",
            "actor",
            "-o",
            "json",
            "--ns-prefix",
            NS_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout",
            "1",
            "--constraint",
            "arch=x86_64",
            "--host-id",
            HOST_ID,
            "wasmcloud.azurecr.io/actor:v1",
        ])?;
        match start_actor_all.command {
            CtlCliCommand::Start(StartCommand::Actor(super::StartActorCommand {
                opts,
                output,
                host_id,
                actor_ref,
                constraints,
            })) => {
                assert_eq!(opts.ctl_host, CTL_HOST);
                assert_eq!(opts.ctl_port, CTL_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(host_id.unwrap(), HOST_ID.to_string());
                assert_eq!(actor_ref, "wasmcloud.azurecr.io/actor:v1".to_string());
                assert_eq!(constraints.unwrap(), vec!["arch=x86_64".to_string()]);
            }
            cmd => panic!("ctl start actor constructed incorrect command {:?}", cmd),
        }
        let start_provider_all = CtlCli::from_iter_safe(&[
            "ctl",
            "start",
            "provider",
            "-o",
            "json",
            "--ns-prefix",
            NS_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout",
            "1",
            "--constraint",
            "arch=x86_64",
            "--host-id",
            HOST_ID,
            "--link-name",
            "default",
            "wasmcloud.azurecr.io/provider:v1",
        ])?;
        match start_provider_all.command {
            CtlCliCommand::Start(StartCommand::Provider(super::StartProviderCommand {
                opts,
                output,
                host_id,
                provider_ref,
                link_name,
                constraints,
            })) => {
                assert_eq!(opts.ctl_host, CTL_HOST);
                assert_eq!(opts.ctl_port, CTL_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(link_name, "default".to_string());
                assert_eq!(constraints.unwrap(), vec!["arch=x86_64".to_string()]);
                assert_eq!(host_id.unwrap(), HOST_ID.to_string());
                assert_eq!(provider_ref, "wasmcloud.azurecr.io/provider:v1".to_string());
            }
            cmd => panic!("ctl start provider constructed incorrect command {:?}", cmd),
        }
        let stop_actor_all = CtlCli::from_iter_safe(&[
            "ctl",
            "stop",
            "actor",
            "-o",
            "json",
            "--ns-prefix",
            NS_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout",
            "1",
            "--count",
            "2",
            HOST_ID,
            ACTOR_ID,
        ])?;
        match stop_actor_all.command {
            CtlCliCommand::Stop(StopCommand::Actor(super::StopActorCommand {
                opts,
                output,
                host_id,
                actor_id,
                count,
            })) => {
                assert_eq!(opts.ctl_host, CTL_HOST);
                assert_eq!(opts.ctl_port, CTL_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(host_id, HOST_ID.to_string());
                assert_eq!(actor_id, ACTOR_ID.to_string());
                assert_eq!(count, 2);
            }
            cmd => panic!("ctl stop actor constructed incorrect command {:?}", cmd),
        }
        let stop_provider_all = CtlCli::from_iter_safe(&[
            "ctl",
            "stop",
            "provider",
            "-o",
            "json",
            "--ns-prefix",
            NS_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout",
            "1",
            HOST_ID,
            PROVIDER_ID,
            "default",
            "wasmcloud:provider",
        ])?;
        match stop_provider_all.command {
            CtlCliCommand::Stop(StopCommand::Provider(super::StopProviderCommand {
                opts,
                output,
                host_id,
                provider_id,
                link_name,
                contract_id,
            })) => {
                assert_eq!(opts.ctl_host, CTL_HOST);
                assert_eq!(opts.ctl_port, CTL_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(host_id, HOST_ID.to_string());
                assert_eq!(provider_id, PROVIDER_ID.to_string());
                assert_eq!(link_name, "default".to_string());
                assert_eq!(contract_id, "wasmcloud:provider".to_string());
            }
            cmd => panic!("ctl stop actor constructed incorrect command {:?}", cmd),
        }
        let get_hosts_all = CtlCli::from_iter_safe(&[
            "ctl",
            "get",
            "hosts",
            "-o",
            "json",
            "--ns-prefix",
            NS_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout",
            "1",
        ])?;
        match get_hosts_all.command {
            CtlCliCommand::Get(GetCommand::Hosts(GetHostsCommand { opts, output })) => {
                assert_eq!(opts.ctl_host, CTL_HOST);
                assert_eq!(opts.ctl_port, CTL_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
            }
            cmd => panic!("ctl get hosts constructed incorrect command {:?}", cmd),
        }
        let get_host_inventory_all = CtlCli::from_iter_safe(&[
            "ctl",
            "get",
            "inventory",
            "-o",
            "json",
            "--ns-prefix",
            NS_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout",
            "1",
            HOST_ID,
        ])?;
        match get_host_inventory_all.command {
            CtlCliCommand::Get(GetCommand::HostInventory(GetHostInventoryCommand {
                opts,
                output,
                host_id,
            })) => {
                assert_eq!(opts.ctl_host, CTL_HOST);
                assert_eq!(opts.ctl_port, CTL_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(host_id, HOST_ID.to_string());
            }
            cmd => panic!("ctl get inventory constructed incorrect command {:?}", cmd),
        }
        let get_claims_all = CtlCli::from_iter_safe(&[
            "ctl",
            "get",
            "claims",
            "-o",
            "json",
            "--ns-prefix",
            NS_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout",
            "1",
        ])?;
        match get_claims_all.command {
            CtlCliCommand::Get(GetCommand::Claims(GetClaimsCommand { opts, output })) => {
                assert_eq!(opts.ctl_host, CTL_HOST);
                assert_eq!(opts.ctl_port, CTL_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
            }
            cmd => panic!("ctl get claims constructed incorrect command {:?}", cmd),
        }
        let link_all = CtlCli::from_iter_safe(&[
            "ctl",
            "link",
            "-o",
            "json",
            "--ns-prefix",
            NS_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout",
            "1",
            "--link-name",
            "default",
            ACTOR_ID,
            PROVIDER_ID,
            "wasmcloud:provider",
            "THING=foo",
        ])?;
        match link_all.command {
            CtlCliCommand::Link(LinkCommand {
                opts,
                output,
                actor_id,
                provider_id,
                contract_id,
                link_name,
                values,
            }) => {
                assert_eq!(opts.ctl_host, CTL_HOST);
                assert_eq!(opts.ctl_port, CTL_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(actor_id, ACTOR_ID.to_string());
                assert_eq!(provider_id, PROVIDER_ID.to_string());
                assert_eq!(contract_id, "wasmcloud:provider".to_string());
                assert_eq!(link_name.unwrap(), "default".to_string());
                assert_eq!(values, vec!["THING=foo".to_string()]);
            }
            cmd => panic!("ctl get claims constructed incorrect command {:?}", cmd),
        }
        let update_all = CtlCli::from_iter_safe(&[
            "ctl",
            "update",
            "actor",
            "-o",
            "json",
            "--ns-prefix",
            NS_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout",
            "1",
            HOST_ID,
            ACTOR_ID,
            "wasmcloud.azurecr.io/actor:v2",
        ])?;
        match update_all.command {
            CtlCliCommand::Update(UpdateCommand::Actor(super::UpdateActorCommand {
                opts,
                output,
                host_id,
                actor_id,
                new_actor_ref,
            })) => {
                assert_eq!(opts.ctl_host, CTL_HOST);
                assert_eq!(opts.ctl_port, CTL_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(host_id, HOST_ID.to_string());
                assert_eq!(actor_id, ACTOR_ID.to_string());
                assert_eq!(new_actor_ref, "wasmcloud.azurecr.io/actor:v2".to_string());
            }
            cmd => panic!("ctl get claims constructed incorrect command {:?}", cmd),
        }

        Ok(())
    }
}
