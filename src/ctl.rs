extern crate wasmcloud_control_interface;
use crate::util::{
    convert_error, format_output, json_str_to_msgpack_bytes, labels_vec_to_hashmap,
    output_destination, Output, OutputDestination, OutputKind, Result, WASH_CMD_INFO,
};
use log::debug;
use serde_json::json;
use spinners::{Spinner, Spinners};
use std::time::Duration;
use structopt::StructOpt;
use term_table::row::Row;
use term_table::table_cell::*;
use term_table::{Table, TableStyle};
use wasmcloud_control_interface::*;

//TODO(brooksmtownsend): If theres a deadline that elapses, suggest specifying a namespace

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
    actor_id: String,

    /// Operation to invoke on actor
    #[structopt(name = "operation")]
    operation: String,

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
    contract_id: String,

    /// Link name, defaults to "default"
    #[structopt(short = "l", long = "link-name")]
    link_name: Option<String>,

    /// Environment values to provide alongside link
    #[structopt(name = "values")]
    values: Vec<String>,
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
    host_id: String,
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
    host_id: Option<String>,

    /// Actor reference, e.g. the OCI URL for the actor
    #[structopt(name = "actor-ref")]
    pub(crate) actor_ref: String,

    /// Constraints for actor auction in the form of "label=value". If host-id is supplied, this list is ignored
    #[structopt(short = "c", long = "constraint", name = "constraints")]
    constraints: Option<Vec<String>>,

    /// Timeout to wait for actor start acknowledgement, defaults to 1 second
    #[structopt(long = "timeout", default_value = "1")]
    timeout: u64,
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
    link_name: String,

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
    link_name: String,

    /// Capability contract Id of provider
    #[structopt(name = "contract-id")]
    contract_id: String,
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct UpdateActorCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(flatten)]
    pub(crate) output: Output,

    /// Id of host
    #[structopt(name = "host-id")]
    host_id: String,

    /// Actor Id, e.g. the public key for the actor
    #[structopt(name = "actor-id")]
    pub(crate) actor_id: String,

    /// Actor reference, e.g. the OCI URL for the actor
    #[structopt(name = "new-actor-ref")]
    pub(crate) new_actor_ref: String,
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
            match ir.error {
                Some(e) => format_output(
                    format!("\nError invoking actor: {}", e),
                    json!({ "error": e }),
                    &output,
                ),
                None => {
                    //TODO(brooksmtownsend): String::from_utf8_lossy should be decoder only if one is not available
                    let call_response = String::from_utf8_lossy(&ir.msg);
                    format_output(
                        format!("\nCall response (raw): {}", call_response),
                        json!({ "response": call_response }),
                        &output,
                    )
                }
            }
        }
        Get(GetCommand::Hosts(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(sp, " Retrieving Hosts ...".to_string(), &output);
            let hosts = get_hosts(cmd).await?;
            debug!(target: WASH_CMD_INFO, "Hosts:{:?}", hosts);
            match output.kind {
                OutputKind::Text => hosts_table(hosts, None),
                OutputKind::JSON => format!("{}", json!({ "hosts": hosts })),
            }
        }
        Get(GetCommand::HostInventory(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(
                sp,
                format!(" Retrieving inventory for host {} ...", cmd.host_id),
                &output,
            );
            let inv = get_host_inventory(cmd).await?;
            debug!(target: WASH_CMD_INFO, "Inventory:{:?}", inv);
            match output.kind {
                OutputKind::Text => host_inventory_table(inv, None),
                OutputKind::JSON => format!("{}", json!({ "inventory": inv })),
            }
        }
        Get(GetCommand::Claims(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(sp, " Retrieving claims ... ".to_string(), &output);
            let claims = get_claims(cmd).await?;
            debug!(target: WASH_CMD_INFO, "Claims:{:?}", claims);
            match output.kind {
                OutputKind::Text => claims_table(claims, None),
                OutputKind::JSON => format!("{}", json!({ "claims": claims })),
            }
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
            debug!(
                target: WASH_CMD_INFO,
                "Publishing link between {} and {}", cmd.actor_id, cmd.provider_id
            );
            match advertise_link(cmd.clone()).await {
                Ok(_) => format_output(
                    format!(
                        "\nAdvertised link ({}) <-> ({}) successfully",
                        cmd.actor_id, cmd.provider_id
                    ),
                    json!({"actor_id": cmd.actor_id, "provider_id": cmd.provider_id, "result": "published"}),
                    &cmd.output,
                ),
                Err(e) => format_output(
                    format!("\nError advertising link: {}", e),
                    json!({ "error": format!("{}", e) }),
                    &cmd.output,
                ),
            }
        }
        Start(StartCommand::Actor(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(
                sp,
                format!(" Starting actor {} ... ", cmd.actor_ref),
                &output,
            );
            debug!(
                target: WASH_CMD_INFO,
                "Sending request to start actor {}", cmd.actor_ref
            );
            match start_actor(cmd).await {
                Ok(r) => format_output(
                    format!("\nActor starting on host {}", r.host_id),
                    json!({ "ack": r }),
                    &output,
                ),
                Err(e) => format_output(
                    format!("\nError starting actor: {}", e),
                    json!({ "error": format!("{}", e) }),
                    &output,
                ),
            }
        }
        Start(StartCommand::Provider(cmd)) => {
            let output = cmd.output;
            sp = update_spinner_message(
                sp,
                format!(" Starting provider {} ... ", cmd.provider_ref),
                &output,
            );
            debug!(
                target: WASH_CMD_INFO,
                "Sending request to start provider {}", cmd.provider_ref
            );
            match start_provider(cmd).await {
                Ok(r) => format_output(
                    format!("\nProvider starting on host {}", r.host_id),
                    json!({ "ack": r }),
                    &output,
                ),
                Err(e) => format_output(
                    format!("\nError starting provider: {}", e),
                    json!({ "error": format!("{}", e) }),
                    &output,
                ),
            }
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
            match ack.failure {
                Some(f) => format_output(
                    format!("\nError stopping actor: {}", f),
                    json!({ "error": f }),
                    &output,
                ),
                None => format_output(
                    format!("\nStopping actor: {}", cmd.actor_id),
                    json!({ "ack": ack }),
                    &output,
                ),
            }
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
            match ack.failure {
                Some(f) => format_output(
                    format!("\nError stopping provider: {}", f),
                    json!({ "error": f }),
                    &output,
                ),
                None => format_output(
                    format!("\nStopping provider: {}", cmd.provider_id),
                    json!({ "ack": ack }),
                    &output,
                ),
            }
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
            match update_actor(cmd.clone()).await {
                Ok(r) => format_output(
                    format!("\nActor {} updated to {}", cmd.actor_id, cmd.new_actor_ref),
                    json!({ "ack": r }),
                    &output,
                ),
                Err(e) => format_output(
                    format!("\nError updating actor: {}", e),
                    json!({ "error": format!("{}", e) }),
                    &output,
                ),
            }
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
    ns_prefix: String,
    timeout: Duration,
) -> Result<Client> {
    let nc = nats::asynk::connect(&format!("{}:{}", host, port)).await?;
    Ok(Client::new(nc, Some(ns_prefix), timeout))
}

async fn client_from_opts(opts: ConnectionOpts) -> Result<Client> {
    new_ctl_client(
        &opts.rpc_host,
        &opts.rpc_port,
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

/// Helper function to print a Host list to stdout as a table
pub(crate) fn hosts_table(hosts: Vec<Host>, max_width: Option<usize>) -> String {
    let mut table = Table::new();
    table.max_column_width = max_width.unwrap_or(80);
    table.style = TableStyle::blank();
    table.separate_rows = false;

    table.add_row(Row::new(vec![
        TableCell::new_with_alignment("Host ID", 1, Alignment::Left),
        TableCell::new_with_alignment("Uptime (seconds)", 1, Alignment::Left),
    ]));
    hosts.iter().for_each(|h| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment(h.id.clone(), 1, Alignment::Left),
            TableCell::new_with_alignment(format!("{}", h.uptime_seconds), 1, Alignment::Left),
        ]))
    });

    table.render()
}

/// Helper function to print a HostInventory to stdout as a table
pub(crate) fn host_inventory_table(inv: HostInventory, max_width: Option<usize>) -> String {
    let mut table = Table::new();
    table.max_column_width = max_width.unwrap_or(80);
    table.style = TableStyle::blank();
    table.separate_rows = false;

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        format!("Host Inventory ({})", inv.host_id),
        4,
        Alignment::Center,
    )]));

    if !inv.labels.is_empty() {
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "",
            4,
            Alignment::Center,
        )]));
        inv.labels.iter().for_each(|(k, v)| {
            table.add_row(Row::new(vec![
                TableCell::new_with_alignment(k, 2, Alignment::Left),
                TableCell::new_with_alignment(v, 2, Alignment::Left),
            ]))
        });
    } else {
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "No labels present",
            4,
            Alignment::Center,
        )]));
    }

    if !inv.actors.is_empty() {
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "",
            4,
            Alignment::Center,
        )]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Actor ID", 2, Alignment::Left),
            TableCell::new_with_alignment("Image Reference", 2, Alignment::Left),
        ]));
        inv.actors.iter().for_each(|a| {
            let a = a.clone();
            table.add_row(Row::new(vec![
                TableCell::new_with_alignment(a.id, 2, Alignment::Left),
                TableCell::new_with_alignment(
                    a.image_ref.unwrap_or_else(|| "N/A".to_string()),
                    2,
                    Alignment::Left,
                ),
            ]))
        });
    } else {
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "No actors found",
            4,
            Alignment::Center,
        )]));
    }

    if !inv.providers.is_empty() {
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "",
            4,
            Alignment::Center,
        )]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Provider ID", 2, Alignment::Left),
            TableCell::new_with_alignment("Link Name", 1, Alignment::Left),
            TableCell::new_with_alignment("Image Reference", 1, Alignment::Left),
        ]));
        inv.providers.iter().for_each(|p| {
            let p = p.clone();
            table.add_row(Row::new(vec![
                TableCell::new_with_alignment(p.id, 2, Alignment::Left),
                TableCell::new_with_alignment(p.link_name, 1, Alignment::Left),
                TableCell::new_with_alignment(
                    p.image_ref.unwrap_or_else(|| "N/A".to_string()),
                    1,
                    Alignment::Left,
                ),
            ]))
        });
    } else {
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "No providers found",
            4,
            Alignment::Left,
        )]));
    }

    table.render()
}

/// Helper function to print a ClaimsList to stdout as a table
pub(crate) fn claims_table(list: ClaimsList, max_width: Option<usize>) -> String {
    let mut table = Table::new();
    table.style = TableStyle::blank();
    table.separate_rows = false;
    table.max_column_width = max_width.unwrap_or(80);

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        "Claims",
        2,
        Alignment::Center,
    )]));

    list.claims.iter().for_each(|c| {
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Issuer", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.values.get("iss").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Subject", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.values.get("sub").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Capabilities", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.values.get("caps").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Version", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.values.get("version").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new_with_alignment("Revision", 1, Alignment::Left),
            TableCell::new_with_alignment(
                c.values.get("rev").unwrap_or(&"".to_string()),
                1,
                Alignment::Left,
            ),
        ]));
        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            format!(""),
            2,
            Alignment::Center,
        )]));
    });

    table.render()
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
    } else if output.kind == OutputKind::Text && output_destination() == OutputDestination::CLI {
        Some(Spinner::new(Spinners::Dots12, msg))
    } else {
        None
    }
}
