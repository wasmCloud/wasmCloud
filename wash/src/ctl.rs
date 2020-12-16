extern crate control_interface;
use control_interface::*;
use spinners::{Spinner, Spinners};
use std::time::Duration;
use structopt::StructOpt;
use term_table::row::Row;
use term_table::table_cell::*;
use term_table::{Table, TableStyle};

use crate::util::{convert_error, json_str_to_msgpack_bytes, labels_vec_to_hashmap, Result};

//TODO(brooksmtownsend): If theres a deadline that elapses, suggest specifying a namespace

#[derive(Debug, Clone, StructOpt)]
pub struct CtlCli {
    #[structopt(flatten)]
    command: CtlCliCommand,
}

#[derive(Debug, Clone, StructOpt)]
pub struct ConnectionOpts {
    /// RPC Host for connection, defaults to 0.0.0.0 for local nats
    #[structopt(short = "r", long = "rpc-host", default_value = "0.0.0.0")]
    rpc_host: String,

    /// RPC Port for connections, defaults to 4222 for local nats
    #[structopt(short = "p", long = "rpc-port", default_value = "4222")]
    rpc_port: String,

    /// Namespace prefix for wasmCloud command interface
    #[structopt(short = "n", long = "ns-prefix", default_value = "default")]
    ns_prefix: String,

    /// Timeout length for RPC, defaults to 5 seconds
    #[structopt(short = "t", long = "rpc-timeout", default_value = "5")]
    rpc_timeout: u64,
}

#[derive(Debug, Clone, StructOpt)]
pub enum CtlCliCommand {
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
}

#[derive(StructOpt, Debug, Clone)]
pub struct CallCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    /// Public key or OCI reference of actor
    #[structopt(name = "actor-id")]
    actor_id: String,

    /// Operation to invoke on actor
    #[structopt(name = "operation")]
    operation: String,

    /// Payload to send with operation (in the form of '{"field": "value"}' )
    #[structopt(name = "data")]
    pub data: Vec<String>,
}

#[derive(Debug, Clone, StructOpt)]
pub enum GetCommand {
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
pub struct LinkCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

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
pub enum StartCommand {
    /// Launch an actor in a host
    #[structopt(name = "actor")]
    Actor(StartActorCommand),

    /// Launch a provider in a host
    #[structopt(name = "provider")]
    Provider(StartProviderCommand),
}

#[derive(Debug, Clone, StructOpt)]
pub enum StopCommand {
    /// Stop an actor running in a host
    #[structopt(name = "actor")]
    Actor(StopActorCommand),

    /// Stop a provider running in a host
    #[structopt(name = "provider")]
    Provider(StopProviderCommand),
}

#[derive(Debug, Clone, StructOpt)]
pub struct GetHostsCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    #[structopt(short = "o", long = "timeout", default_value = "2")]
    timeout: u64,
}

#[derive(Debug, Clone, StructOpt)]
pub struct GetHostInventoryCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    /// Id of host
    #[structopt(name = "host-id")]
    host_id: String,
}

#[derive(Debug, Clone, StructOpt)]
pub struct GetClaimsCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,
}

#[derive(Debug, Clone, StructOpt)]
pub struct StartActorCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    /// Id of host, if omitted the actor will be auctioned in the lattice to find a suitable host
    #[structopt(short = "h", long = "host-id", name = "host-id")]
    host_id: Option<String>,

    /// Actor reference, e.g. the OCI URL for the actor
    #[structopt(name = "actor-ref")]
    pub(crate) actor_ref: String,

    /// Constraints for actor auction in the form of "label=value". If host-id is supplied, this list is ignored
    #[structopt(short = "c", long = "constraint", name = "constraints")]
    constraints: Option<Vec<String>>,

    #[structopt(short = "o", long = "timeout", default_value = "2")]
    timeout: u64,
}

#[derive(Debug, Clone, StructOpt)]
pub struct StartProviderCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

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

    #[structopt(short = "o", long = "timeout", default_value = "5")]
    timeout: u64,
}

#[derive(Debug, Clone, StructOpt)]
pub struct StopActorCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    /// Id of host
    #[structopt(name = "host-id")]
    pub(crate) host_id: String,

    /// Actor reference, e.g. the OCI URL for the actor
    #[structopt(name = "actor-ref")]
    pub(crate) actor_ref: String,
}

#[derive(Debug, Clone, StructOpt)]
pub struct StopProviderCommand {
    #[structopt(flatten)]
    opts: ConnectionOpts,

    /// Id of host
    #[structopt(name = "host-id")]
    host_id: String,

    /// Provider reference, e.g. the OCI URL for the provider
    #[structopt(name = "provider-ref")]
    pub(crate) provider_ref: String,

    /// Link name of provider
    #[structopt(name = "link-name")]
    link_name: String,

    /// Capability contract Id of provider
    #[structopt(name = "contract-id")]
    contract_id: String,
}

pub(crate) async fn handle_command(cli: CtlCli) -> Result<()> {
    use CtlCliCommand::*;
    let sp: Spinner = Spinner::new(Spinners::Dots12, "".to_string());
    let output = match cli.command {
        Call(cmd) => {
            sp.message(format!(" Calling Actor {} ...", cmd.actor_id));
            let ir = call_actor(cmd).await?;
            match ir.error {
                Some(e) => format!("Error invoking actor: {}", e),
                None => {
                    //TODO(brooksmtownsend): String::from_utf8_lossy should be decoder only if one is not available
                    format!("Call response (raw): {}", String::from_utf8_lossy(&ir.msg))
                }
            }
        }
        Get(GetCommand::Hosts(cmd)) => {
            sp.message(format!(" Retrieving Hosts ..."));
            let hosts = get_hosts(cmd).await?;
            format!("{}", hosts_table(hosts, None))
        }
        Get(GetCommand::HostInventory(cmd)) => {
            sp.message(format!(
                " Retrieving inventory for host {} ...",
                cmd.host_id
            ));
            let inv = get_host_inventory(cmd).await?;
            format!("{}", host_inventory_table(inv, None))
        }
        Get(GetCommand::Claims(cmd)) => {
            sp.message(format!(" Retrieving claims ... "));
            let claims = get_claims(cmd).await?;
            format!("{}", claims_table(claims, None))
        }
        Link(cmd) => {
            sp.message(format!(
                " Advertising link between {} and {} ... ",
                cmd.actor_id, cmd.provider_id
            ));
            match advertise_link(cmd.clone()).await {
                Ok(_) => format!(
                    "Advertised link ({}) <-> ({}) successfully",
                    cmd.actor_id, cmd.provider_id
                ),
                Err(e) => format!("Error advertising link: {}", e),
            }
        }
        Start(StartCommand::Actor(cmd)) => {
            sp.message(format!(" Starting actor {} ... ", cmd.actor_ref));
            match start_actor(cmd).await {
                Ok(r) => format!("Actor {} being scheduled on host {}", r.actor_id, r.host_id),
                Err(e) => format!("Error starting actor: {}", e),
            }
        }
        Start(StartCommand::Provider(cmd)) => {
            sp.message(format!(" Starting provider {} ... ", cmd.provider_ref));
            match start_provider(cmd).await {
                Ok(r) => format!(
                    "Provider {} being scheduled on host {}",
                    r.provider_id, r.host_id
                ),
                Err(e) => format!("Error starting provider: {}", e),
            }
        }
        Stop(StopCommand::Actor(cmd)) => {
            sp.message(format!(" Stopping actor {} ... ", cmd.actor_ref));
            match stop_actor(cmd.clone()).await?.failure {
                Some(f) => format!("Error stopping actor: {}", f),
                None => format!("Stopping actor: {}", cmd.actor_ref),
            }
        }
        Stop(StopCommand::Provider(cmd)) => {
            sp.message(format!(" Stopping provider {} ... ", cmd.provider_ref));
            match stop_provider(cmd.clone()).await?.failure {
                Some(f) => format!("Error stopping provider: {}", f),
                None => format!("Stopping provider: {}", cmd.provider_ref),
            }
        }
    };

    sp.stop();
    println!("\n{}", output);

    Ok(())
}

pub async fn new_ctl_client(
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

pub async fn call_actor(cmd: CallCommand) -> Result<InvocationResponse> {
    let client = client_from_opts(cmd.opts).await?;
    let bytes = json_str_to_msgpack_bytes(cmd.data)?;
    client
        .call_actor(&cmd.actor_id, &cmd.operation, &bytes)
        .await
        .map_err(convert_error)
}

pub async fn get_hosts(cmd: GetHostsCommand) -> Result<Vec<Host>> {
    let timeout = Duration::from_secs(cmd.timeout);
    let client = client_from_opts(cmd.opts).await?;
    client.get_hosts(timeout).await.map_err(convert_error)
}

pub async fn get_host_inventory(cmd: GetHostInventoryCommand) -> Result<HostInventory> {
    let client = client_from_opts(cmd.opts).await?;
    client
        .get_host_inventory(&cmd.host_id)
        .await
        .map_err(convert_error)
}

pub async fn get_claims(cmd: GetClaimsCommand) -> Result<ClaimsList> {
    let client = client_from_opts(cmd.opts).await?;
    client.get_claims().await.map_err(convert_error)
}

pub async fn advertise_link(cmd: LinkCommand) -> Result<()> {
    let client = client_from_opts(cmd.opts).await?;
    client
        .advertise_link(
            &cmd.actor_id,
            &cmd.provider_id,
            &cmd.contract_id,
            &cmd.link_name.unwrap_or("default".to_string()),
            labels_vec_to_hashmap(cmd.values)?,
        )
        .await
        .map_err(convert_error)
}

pub async fn start_actor(cmd: StartActorCommand) -> Result<StartActorAck> {
    let client = client_from_opts(cmd.opts.clone()).await?;

    let host = match cmd.host_id {
        Some(host) => host,
        None => {
            let suitable_hosts = client
                .perform_actor_auction(
                    &cmd.actor_ref,
                    labels_vec_to_hashmap(cmd.constraints.unwrap_or(vec![]))?,
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

pub async fn start_provider(cmd: StartProviderCommand) -> Result<StartProviderAck> {
    let client = client_from_opts(cmd.opts.clone()).await?;

    let host = match cmd.host_id {
        Some(host) => host,
        None => {
            let suitable_hosts = client
                .perform_provider_auction(
                    &cmd.provider_ref,
                    &cmd.link_name,
                    labels_vec_to_hashmap(cmd.constraints.unwrap_or(vec![]))?,
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

pub async fn stop_provider(cmd: StopProviderCommand) -> Result<StopProviderAck> {
    let client = client_from_opts(cmd.opts).await?;
    client
        .stop_provider(
            &cmd.host_id,
            &cmd.provider_ref,
            &cmd.link_name,
            &cmd.contract_id,
        )
        .await
        .map_err(convert_error)
}

pub async fn stop_actor(cmd: StopActorCommand) -> Result<StopActorAck> {
    let client = client_from_opts(cmd.opts).await?;
    client
        .stop_actor(&cmd.host_id, &cmd.actor_ref)
        .await
        .map_err(convert_error)
}

/// Helper function to print a Host list to stdout as a table
pub(crate) fn hosts_table(hosts: Vec<Host>, max_width: Option<usize>) -> String {
    let mut table = Table::new();
    table.max_column_width = match max_width {
        Some(n) => n,
        None => 80,
    };
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
    table.max_column_width = match max_width {
        Some(n) => n,
        None => 80,
    };
    table.style = TableStyle::blank();
    table.separate_rows = false;

    table.add_row(Row::new(vec![TableCell::new_with_alignment(
        format!("Host Inventory ({})", inv.host_id),
        4,
        Alignment::Center,
    )]));

    if inv.labels.len() >= 1 {
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

    if inv.actors.len() >= 1 {
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
                    a.image_ref.unwrap_or("N/A".to_string()),
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

    if inv.providers.len() >= 1 {
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
                    p.image_ref.unwrap_or("N/A".to_string()),
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
    table.max_column_width = match max_width {
        Some(n) => n,
        None => 80,
    };

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
