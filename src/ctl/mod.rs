extern crate wasmcloud_control_interface;
use crate::util::{
    convert_error, extract_arg_value, json_str_to_msgpack_bytes, labels_vec_to_hashmap,
    output_destination, Output, OutputDestination, OutputKind, Result, WASH_CMD_INFO,
};
use log::debug;
use spinners::{Spinner, Spinners};
use std::time::Duration;
use structopt::StructOpt;
use wasmcloud_control_interface::*;
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
    /// RPC Host for connection, defaults to 0.0.0.0 for local nats
    #[structopt(
        short = "r",
        long = "rpc-host",
        default_value = "0.0.0.0",
        env = "WASH_RPC_HOST"
    )]
    rpc_host: String,

    /// RPC Port for connections, defaults to 4222 for local nats
    #[structopt(
        short = "p",
        long = "rpc-port",
        default_value = "4222",
        env = "WASH_RPC_PORT"
    )]
    rpc_port: String,

    /// JWT file for RPC authentication. Must be supplied with rpc_seed.
    #[structopt(long = "rpc-jwt", env = "WASH_RPC_JWT", hide_env_values = true)]
    rpc_jwt: Option<String>,

    /// Seed file or literal for RPC authentication. Must be supplied with rpc_jwt.
    #[structopt(long = "rpc-seed", env = "WASH_RPC_SEED", hide_env_values = true)]
    rpc_seed: Option<String>,

    /// Credsfile for RPC authentication. Combines rpc_seed and rpc_jwt.
    /// See https://docs.nats.io/developing-with-nats/security/creds for details.
    #[structopt(long = "rpc-credsfile", env = "WASH_RPC_CREDS", hide_env_values = true)]
    rpc_credsfile: Option<String>,

    /// Namespace prefix for wasmcloud command interface
    #[structopt(short = "n", long = "ns-prefix", default_value = "default")]
    ns_prefix: String,

    /// Timeout length for RPC, defaults to 1 second
    #[structopt(
        short = "t",
        long = "rpc-timeout",
        default_value = "1",
        env = "WASH_RPC_TIMEOUT"
    )]
    rpc_timeout: u64,
}

impl Default for ConnectionOpts {
    fn default() -> Self {
        ConnectionOpts {
            rpc_host: "0.0.0.0".to_string(),
            rpc_port: "4222".to_string(),
            rpc_jwt: None,
            rpc_seed: None,
            rpc_credsfile: None,
            ns_prefix: "default".to_string(),
            rpc_timeout: 1,
        }
    }
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) enum CtlCliCommand {
    /// Invoke an operation on an actor
    #[structopt(name = "call")]
    Call(CallCommand),

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

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct CallCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(flatten)]
    pub(crate) output: Output,

    /// Public key or OCI reference of actor
    #[structopt(name = "actor-id")]
    pub(crate) actor_id: String,

    /// Operation to invoke on actor
    #[structopt(name = "operation")]
    pub(crate) operation: String,

    /// Payload to send with operation (in the form of '{"field": "value"}' )
    #[structopt(name = "data")]
    pub(crate) data: Vec<String>,
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

    #[structopt(long = "timeout", default_value = "1")]
    timeout: u64,
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

    /// Timeout to wait for actor start acknowledgement, defaults to 1 second
    #[structopt(long = "timeout", default_value = "1")]
    timeout: u64,
}

impl StartActorCommand {
    pub(crate) fn new(
        opts: ConnectionOpts,
        output: Output,
        host_id: Option<String>,
        actor_ref: String,
        constraints: Option<Vec<String>>,
        timeout: u64,
    ) -> Self {
        StartActorCommand {
            opts,
            output,
            host_id,
            actor_ref,
            constraints,
            timeout,
        }
    }
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

    /// Timeout to wait for provider start acknowledgement, defaults to 1 second
    #[structopt(long = "timeout", default_value = "1")]
    timeout: u64,
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

impl UpdateActorCommand {
    pub(crate) fn new(
        opts: ConnectionOpts,
        output: Output,
        host_id: String,
        actor_id: String,
        new_actor_ref: String,
    ) -> Self {
        UpdateActorCommand {
            opts,
            output,
            host_id,
            actor_id,
            new_actor_ref,
        }
    }
}

pub(crate) async fn handle_command(command: CtlCliCommand) -> Result<String> {
    use CtlCliCommand::*;
    let mut sp: Option<Spinner> = None;
    let out = match command {
        Call(cmd) => {
            let output = cmd.output;
            sp =
                update_spinner_message(sp, format!("Calling actor {} ... ", cmd.actor_id), &output);
            debug!(target: WASH_CMD_INFO, "Calling actor {}", cmd.actor_id);
            let ir = call_actor(cmd).await?;
            debug!(target: WASH_CMD_INFO, "Invocation response {:?}", ir);
            call_output(ir.error, ir.msg, &output.kind)
        }
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
            sp = update_spinner_message(
                sp,
                format!(" Starting actor {} ... ", cmd.actor_ref),
                &output,
            );
            let ack = start_actor(cmd).await?;
            start_actor_output(&ack.actor_ref, &ack.host_id, ack.failure, &output.kind)
        }
        Start(StartCommand::Provider(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(
                sp,
                format!(" Starting provider {} ... ", cmd.provider_ref),
                &output,
            );
            let ack = start_provider(cmd).await?;
            start_provider_output(&ack.provider_ref, &ack.host_id, ack.failure, &output.kind)
        }
        Stop(StopCommand::Actor(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(
                sp,
                format!(" Stopping actor {} ... ", cmd.actor_id),
                &output,
            );
            let ack = stop_actor(cmd.clone()).await?;
            debug!(target: WASH_CMD_INFO, "Stop actor ack: {:?}", ack);
            stop_actor_output(&cmd.actor_id, ack.failure, &cmd.output.kind)
        }
        Stop(StopCommand::Provider(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(
                sp,
                format!(" Stopping provider {} ... ", cmd.provider_id),
                &output,
            );
            let ack = stop_provider(cmd.clone()).await?;
            debug!(target: WASH_CMD_INFO, "Stop provider ack: {:?}", ack);
            stop_provider_output(&cmd.provider_id, ack.failure, &cmd.output.kind)
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
            debug!(
                "Sending request to update actor {} to {}",
                cmd.actor_id, cmd.new_actor_ref
            );
            let ack = update_actor(cmd.clone()).await;
            update_actor_output(
                &cmd.actor_id,
                &cmd.new_actor_ref,
                ack.map_or_else(|e| Some(format!("{}", e)), |_| None),
                &cmd.output.kind,
            )
        }
    };

    if sp.is_some() {
        sp.unwrap().stop()
    }

    Ok(out)
}

pub(crate) async fn new_ctl_client(
    host: &str,
    port: &str,
    jwt: Option<String>,
    seed: Option<String>,
    credsfile: Option<String>,
    ns_prefix: String,
    timeout: Duration,
) -> Result<Client> {
    let nats_url = format!("{}:{}", host, port);
    let nc = if let (Some(jwt_file), Some(seed_val)) = (jwt, seed) {
        let kp = nkeys::KeyPair::from_seed(&extract_arg_value(&seed_val)?)?;
        let jwt_contents = extract_arg_value(&jwt_file)?;
        // You must provide the JWT via a closure
        nats::Options::with_jwt(
            move || Ok(jwt_contents.clone()),
            move |nonce| kp.sign(nonce).unwrap(),
        )
        .connect_async(&nats_url)
        .await?
    } else if let Some(credsfile_path) = credsfile {
        nats::Options::with_credentials(credsfile_path)
            .connect_async(&nats_url)
            .await?
    } else {
        nats::asynk::connect(&nats_url).await?
    };
    Ok(Client::new(nc, Some(ns_prefix), timeout))
}

async fn client_from_opts(opts: ConnectionOpts) -> Result<Client> {
    new_ctl_client(
        &opts.rpc_host,
        &opts.rpc_port,
        opts.rpc_jwt,
        opts.rpc_seed,
        opts.rpc_credsfile,
        opts.ns_prefix,
        Duration::from_secs(opts.rpc_timeout),
    )
    .await
}

pub(crate) async fn call_actor(cmd: CallCommand) -> Result<InvocationResponse> {
    let client = client_from_opts(cmd.opts).await?;
    let bytes = json_str_to_msgpack_bytes(cmd.data)?;
    client
        .call_actor(&cmd.actor_id, &cmd.operation, &bytes)
        .await
        .map_err(convert_error)
}

pub(crate) async fn get_hosts(cmd: GetHostsCommand) -> Result<Vec<Host>> {
    let timeout = Duration::from_secs(cmd.timeout);
    let client = client_from_opts(cmd.opts).await?;
    client.get_hosts(timeout).await.map_err(convert_error)
}

pub(crate) async fn get_host_inventory(cmd: GetHostInventoryCommand) -> Result<HostInventory> {
    let client = client_from_opts(cmd.opts).await?;
    client
        .get_host_inventory(&cmd.host_id)
        .await
        .map_err(convert_error)
}

pub(crate) async fn get_claims(cmd: GetClaimsCommand) -> Result<ClaimsList> {
    let client = client_from_opts(cmd.opts).await?;
    client.get_claims().await.map_err(convert_error)
}

pub(crate) async fn advertise_link(cmd: LinkCommand) -> Result<()> {
    let client = client_from_opts(cmd.opts).await?;
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

pub(crate) async fn start_actor(cmd: StartActorCommand) -> Result<StartActorAck> {
    let client = client_from_opts(cmd.opts.clone()).await?;

    let host = match cmd.host_id {
        Some(host) => host,
        None => {
            let suitable_hosts = client
                .perform_actor_auction(
                    &cmd.actor_ref,
                    labels_vec_to_hashmap(cmd.constraints.unwrap_or_default())?,
                    Duration::from_secs(cmd.timeout),
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

pub(crate) async fn start_provider(cmd: StartProviderCommand) -> Result<StartProviderAck> {
    let client = client_from_opts(cmd.opts.clone()).await?;

    let host = match cmd.host_id {
        Some(host) => host,
        None => {
            let suitable_hosts = client
                .perform_provider_auction(
                    &cmd.provider_ref,
                    &cmd.link_name,
                    labels_vec_to_hashmap(cmd.constraints.unwrap_or_default())?,
                    Duration::from_secs(cmd.timeout),
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

pub(crate) async fn stop_provider(cmd: StopProviderCommand) -> Result<StopProviderAck> {
    let client = client_from_opts(cmd.opts).await?;
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

pub(crate) async fn stop_actor(cmd: StopActorCommand) -> Result<StopActorAck> {
    let client = client_from_opts(cmd.opts).await?;
    client
        .stop_actor(&cmd.host_id, &cmd.actor_id)
        .await
        .map_err(convert_error)
}

pub(crate) async fn update_actor(cmd: UpdateActorCommand) -> Result<UpdateActorAck> {
    let client = client_from_opts(cmd.opts).await?;
    client
        .update_actor(&cmd.host_id, &cmd.actor_id, &cmd.new_actor_ref)
        .await
        .map_err(convert_error)
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
    } else if matches!(output.kind, OutputKind::Text { .. })
        && output_destination() == OutputDestination::Cli
    {
        Some(Spinner::new(Spinners::Dots12, msg))
    } else {
        None
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const RPC_HOST: &str = "0.0.0.0";
    const RPC_PORT: &str = "4222";
    const NS_PREFIX: &str = "default";

    const ACTOR_ID: &str = "MDPDJEYIAK6MACO67PRFGOSSLODBISK4SCEYDY3HEOY4P5CVJN6UCWUK";
    const PROVIDER_ID: &str = "VBKTSBG2WKP6RJWLQ5O7RDVIIB4LMW6U5R67A7QMIDBZDGZWYTUE3TSI";
    const HOST_ID: &str = "NCE7YHGI42RWEKBRDJZWXBEJJCFNE5YIWYMSTLGHQBEGFY55BKJ3EG3G";

    #[test]
    /// Enumerates multiple options of the `ctl` command to ensure API doesn't
    /// change between versions. This test will fail if any subcommand of `wash ctl`
    /// changes syntax, ordering of required elements, or flags.
    fn test_ctl_comprehensive() -> Result<()> {
        let call_all = CtlCli::from_iter_safe(&[
            "ctl",
            "call",
            "-o",
            "json",
            "--ns-prefix",
            NS_PREFIX,
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout",
            "1",
            ACTOR_ID,
            "HandleOperation",
            "{ \"hello\": \"world\"}",
        ])?;
        match call_all.command {
            CtlCliCommand::Call(CallCommand {
                opts,
                output,
                actor_id,
                operation,
                data,
            }) => {
                assert_eq!(opts.rpc_host, RPC_HOST);
                assert_eq!(opts.rpc_port, RPC_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.rpc_timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(actor_id, ACTOR_ID);
                assert_eq!(operation, "HandleOperation");
                assert_eq!(data, vec!["{ \"hello\": \"world\"}".to_string()])
            }
            cmd => panic!("ctl call constructed incorrect command: {:?}", cmd),
        }
        let start_actor_all = CtlCli::from_iter_safe(&[
            "ctl",
            "start",
            "actor",
            "-o",
            "json",
            "--ns-prefix",
            NS_PREFIX,
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout",
            "1",
            "--constraint",
            "arch=x86_64",
            "--host-id",
            HOST_ID,
            "--timeout",
            "5",
            "wasmcloud.azurecr.io/actor:v1",
        ])?;
        match start_actor_all.command {
            CtlCliCommand::Start(StartCommand::Actor(super::StartActorCommand {
                opts,
                output,
                host_id,
                actor_ref,
                constraints,
                timeout,
            })) => {
                assert_eq!(opts.rpc_host, RPC_HOST);
                assert_eq!(opts.rpc_port, RPC_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.rpc_timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(host_id.unwrap(), HOST_ID.to_string());
                assert_eq!(actor_ref, "wasmcloud.azurecr.io/actor:v1".to_string());
                assert_eq!(constraints.unwrap(), vec!["arch=x86_64".to_string()]);
                assert_eq!(timeout, 5);
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
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout",
            "1",
            "--constraint",
            "arch=x86_64",
            "--host-id",
            HOST_ID,
            "--timeout",
            "5",
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
                timeout,
            })) => {
                assert_eq!(opts.rpc_host, RPC_HOST);
                assert_eq!(opts.rpc_port, RPC_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.rpc_timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(link_name, "default".to_string());
                assert_eq!(constraints.unwrap(), vec!["arch=x86_64".to_string()]);
                assert_eq!(host_id.unwrap(), HOST_ID.to_string());
                assert_eq!(provider_ref, "wasmcloud.azurecr.io/provider:v1".to_string());
                assert_eq!(timeout, 5);
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
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout",
            "1",
            HOST_ID,
            ACTOR_ID,
        ])?;
        match stop_actor_all.command {
            CtlCliCommand::Stop(StopCommand::Actor(super::StopActorCommand {
                opts,
                output,
                host_id,
                actor_id,
            })) => {
                assert_eq!(opts.rpc_host, RPC_HOST);
                assert_eq!(opts.rpc_port, RPC_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.rpc_timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(host_id, HOST_ID.to_string());
                assert_eq!(actor_id, ACTOR_ID.to_string());
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
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout",
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
                assert_eq!(opts.rpc_host, RPC_HOST);
                assert_eq!(opts.rpc_port, RPC_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.rpc_timeout, 1);
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
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout",
            "1",
            "--timeout",
            "5",
        ])?;
        match get_hosts_all.command {
            CtlCliCommand::Get(GetCommand::Hosts(GetHostsCommand {
                opts,
                output,
                timeout,
            })) => {
                assert_eq!(opts.rpc_host, RPC_HOST);
                assert_eq!(opts.rpc_port, RPC_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.rpc_timeout, 1);
                assert_eq!(output.kind, OutputKind::Json);
                assert_eq!(timeout, 5);
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
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout",
            "1",
            HOST_ID,
        ])?;
        match get_host_inventory_all.command {
            CtlCliCommand::Get(GetCommand::HostInventory(GetHostInventoryCommand {
                opts,
                output,
                host_id,
            })) => {
                assert_eq!(opts.rpc_host, RPC_HOST);
                assert_eq!(opts.rpc_port, RPC_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.rpc_timeout, 1);
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
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout",
            "1",
        ])?;
        match get_claims_all.command {
            CtlCliCommand::Get(GetCommand::Claims(GetClaimsCommand { opts, output })) => {
                assert_eq!(opts.rpc_host, RPC_HOST);
                assert_eq!(opts.rpc_port, RPC_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.rpc_timeout, 1);
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
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout",
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
                assert_eq!(opts.rpc_host, RPC_HOST);
                assert_eq!(opts.rpc_port, RPC_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.rpc_timeout, 1);
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
            "--rpc-host",
            RPC_HOST,
            "--rpc-port",
            RPC_PORT,
            "--rpc-timeout",
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
                assert_eq!(opts.rpc_host, RPC_HOST);
                assert_eq!(opts.rpc_port, RPC_PORT);
                assert_eq!(opts.ns_prefix, NS_PREFIX);
                assert_eq!(opts.rpc_timeout, 1);
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
