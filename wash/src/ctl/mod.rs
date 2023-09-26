use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};
use std::path::Path;
use wash_lib::{
    actor::{scale_actor, update_actor},
    cli::{
        get::{GetClaimsCommand, GetCommand, GetHostInventoryCommand, GetHostsCommand},
        labels_vec_to_hashmap,
        link::LinkCommand,
        start::StartCommand,
        stop::{handle_stop_actor, stop_host, stop_provider, StopCommand},
        CliConnectionOpts, CommandOutput, OutputKind,
    },
    config::WashConnectionOptions,
    id::{ModuleId, ServerId},
};
use wasmcloud_control_interface::Client as CtlClient;

use crate::{
    appearance::spinner::Spinner,
    common::link_cmd::handle_command as handle_link_command,
    common::{
        get_cmd::handle_command as handle_get_command,
        start_cmd::handle_command as handle_start_command,
    },
    ctl::manifest::HostManifest,
};
pub(crate) use output::*;

mod manifest;
mod output;

// default start actor command starts with one actor
const ONE_ACTOR: u16 = 1;

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum CtlCliCommand {
    /// Retrieves information about the lattice
    #[clap(name = "get", subcommand)]
    Get(CtlGetCommand),

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
    #[clap(name = "host-key", value_parser)]
    pub(crate) host_key: ServerId,

    /// Path to the manifest file. Note that all the entries in this file are imperative instructions, and all actor and provider references MUST be valid OCI references.
    #[clap(name = "path")]
    pub(crate) path: String,

    /// Expand environment variables using substitution syntax within the manifest file
    #[clap(name = "expand-env", short = 'e', long = "expand-env")]
    pub(crate) expand_env: bool,

    #[clap(flatten)]
    opts: CliConnectionOpts,
}

#[derive(Debug, Clone, Subcommand)]
pub(crate) enum CtlGetCommand {
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
    opts: CliConnectionOpts,

    /// Id of host
    #[clap(name = "host-id", value_parser)]
    host_id: ServerId,

    /// Actor Id, e.g. the public key for the actor
    #[clap(name = "actor-id", value_parser)]
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
pub(crate) struct UpdateActorCommand {
    #[clap(flatten)]
    opts: CliConnectionOpts,

    /// Id of host
    #[clap(name = "host-id", value_parser)]
    pub(crate) host_id: ServerId,

    /// Actor Id, e.g. the public key for the actor
    #[clap(name = "actor-id", value_parser)]
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
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out: CommandOutput = match command {
        Apply(cmd) => {
            sp.update_spinner_message(" Applying manifest ...".to_string());
            let results = apply_manifest(cmd).await?;
            apply_manifest_output(results)
        }
        Get(CtlGetCommand::Hosts(cmd)) => {
            eprintln!("[warn] `wash ctl get hosts` has been deprecated in favor of `wash get hosts` and will be removed in a future version.");
            handle_get_command(GetCommand::Hosts(cmd), output_kind).await?
        }
        Get(CtlGetCommand::HostInventory(cmd)) => {
            eprintln!("[warn] `wash ctl get inventory` has been deprecated in favor of `wash get inventory` and will be removed in a future version.");
            handle_get_command(GetCommand::HostInventory(cmd), output_kind).await?
        }
        Get(CtlGetCommand::Claims(cmd)) => {
            eprintln!("[warn] `wash ctl get claims` has been deprecated in favor of `wash get claims` and will be removed in a future version.");
            handle_get_command(GetCommand::Claims(cmd), output_kind).await?
        }
        Link(cmd) => {
            eprintln!("[warn] `wash ctl link` has been deprecated in favor of `wash link` and will be removed in a future version.");
            handle_link_command(cmd, output_kind).await?
        }
        Start(cmd) => {
            eprintln!("[warn] `wash ctl start` has been deprecated in favor of `wash start` and will be removed in a future version.");
            handle_start_command(cmd, output_kind).await?
        }
        Stop(StopCommand::Actor(cmd)) => {
            eprintln!("[warn] `wash ctl stop` has been deprecated in favor of `wash stop` and will be removed in a future version.");
            sp.update_spinner_message(format!(" Stopping actor {} ... ", cmd.actor_id));
            handle_stop_actor(cmd.clone()).await?
        }
        Stop(StopCommand::Provider(cmd)) => {
            sp.update_spinner_message(format!(" Stopping provider {} ... ", cmd.provider_id));

            stop_provider(cmd.clone()).await?
        }
        Stop(StopCommand::Host(cmd)) => {
            sp.update_spinner_message(format!(" Stopping host {} ... ", cmd.host_id));

            stop_host(cmd.clone()).await?
        }
        Update(UpdateCommand::Actor(cmd)) => {
            sp.update_spinner_message(format!(
                " Updating Actor {} to {} ... ",
                cmd.actor_id, cmd.new_actor_ref
            ));

            let ack = update_actor(
                cmd.opts.try_into()?,
                &cmd.host_id,
                &cmd.actor_id,
                &cmd.new_actor_ref,
            )
            .await?;
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
            handle_scale_actor(cmd.clone()).await?
        }
    };

    sp.finish_and_clear();

    Ok(out)
}

pub(crate) async fn handle_scale_actor(cmd: ScaleActorCommand) -> Result<CommandOutput> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;

    let annotations = labels_vec_to_hashmap(cmd.annotations)?;

    scale_actor(
        &client,
        &cmd.host_id,
        &cmd.actor_ref,
        &cmd.actor_id,
        cmd.count,
        Some(annotations),
    )
    .await?;

    Ok(CommandOutput::from_key_and_text(
        "result",
        format!(
            "Request to scale actor {} to {} instances recieved",
            cmd.actor_id, cmd.count
        ),
    ))
}

pub(crate) async fn apply_manifest(cmd: ApplyCommand) -> Result<Vec<String>> {
    let wco: WashConnectionOptions = cmd.opts.try_into()?;
    let client = wco.into_ctl_client(None).await?;
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
        match client.start_actor(host_id, actor, ONE_ACTOR, None).await {
            Ok(ack) => {
                if ack.accepted {
                    results.push(format!("Instruction to start actor {actor} acknowledged."));
                } else {
                    results.push(format!(
                        "Instruction to start actor {} not acked: {}",
                        actor, ack.error
                    ));
                }
            }
            Err(e) => results.push(format!("Failed to send start actor: {e}")),
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
            Err(e) => results.push(format!("Failed to send link def: {e}")),
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
            .start_provider(host_id, &cap.image_ref, cap.link_name.clone(), None, None)
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
            Err(e) => results.push(format!("Failed to send start capability message: {e}")),
        }
    }

    Ok(results)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::CtlCliCommand;
    use clap::Parser;
    use wash_lib::cli::{
        get::GetHostsCommand,
        stop::{StopActorCommand, StopProviderCommand},
    };

    #[derive(Parser)]
    struct Cmd {
        #[clap(subcommand)]
        command: CtlCliCommand,
    }

    const CTL_HOST: &str = "127.0.0.1";
    const CTL_PORT: &str = "4222";
    const LATTICE_PREFIX: &str = "default";
    const JS_DOMAIN: &str = "custom-domain";

    const ACTOR_ID: &str = "MDPDJEYIAK6MACO67PRFGOSSLODBISK4SCEYDY3HEOY4P5CVJN6UCWUK";
    const PROVIDER_ID: &str = "VBKTSBG2WKP6RJWLQ5O7RDVIIB4LMW6U5R67A7QMIDBZDGZWYTUE3TSI";
    const HOST_ID: &str = "NCE7YHGI42RWEKBRDJZWXBEJJCFNE5YIWYMSTLGHQBEGFY55BKJ3EG3G";

    #[test]
    /// Enumerates multiple options of the `ctl` command to ensure API doesn't
    /// change between versions. This test will fail if any subcommand of `wash ctl`
    /// changes syntax, ordering of required elements, or flags.
    fn test_ctl_comprehensive() -> Result<()> {
        let stop_actor_all: Cmd = Parser::try_parse_from([
            "ctl",
            "stop",
            "actor",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
            "--count",
            "2",
            HOST_ID,
            ACTOR_ID,
        ])?;
        match stop_actor_all.command {
            CtlCliCommand::Stop(StopCommand::Actor(StopActorCommand {
                opts,
                host_id,
                actor_id,
                count,
                skip_wait,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, HOST_ID.parse()?);
                assert_eq!(actor_id, ACTOR_ID.parse()?);
                assert_eq!(count, 2);
                assert!(!skip_wait);
            }
            cmd => panic!("ctl stop actor constructed incorrect command {cmd:?}"),
        }
        let stop_provider_all: Cmd = Parser::try_parse_from([
            "ctl",
            "stop",
            "provider",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
            HOST_ID,
            PROVIDER_ID,
            "default",
            "wasmcloud:provider",
        ])?;
        match stop_provider_all.command {
            CtlCliCommand::Stop(StopCommand::Provider(StopProviderCommand {
                opts,
                host_id,
                provider_id,
                link_name,
                contract_id,
                skip_wait,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, HOST_ID.parse()?);
                assert_eq!(provider_id, PROVIDER_ID.parse()?);
                assert_eq!(link_name, "default".to_string());
                assert_eq!(contract_id, "wasmcloud:provider".to_string());
                assert!(!skip_wait);
            }
            cmd => panic!("ctl stop actor constructed incorrect command {cmd:?}"),
        }
        let get_hosts_all: Cmd = Parser::try_parse_from([
            "ctl",
            "get",
            "hosts",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
        ])?;
        match get_hosts_all.command {
            CtlCliCommand::Get(CtlGetCommand::Hosts(GetHostsCommand { opts })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.timeout_ms, 2001);
            }
            cmd => panic!("ctl get hosts constructed incorrect command {cmd:?}"),
        }
        let get_host_inventory_all: Cmd = Parser::try_parse_from([
            "ctl",
            "get",
            "inventory",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
            HOST_ID,
        ])?;
        match get_host_inventory_all.command {
            CtlCliCommand::Get(CtlGetCommand::HostInventory(GetHostInventoryCommand {
                opts,
                host_id,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id.unwrap(), HOST_ID.parse()?);
            }
            cmd => panic!("ctl get inventory constructed incorrect command {cmd:?}"),
        }
        let get_claims_all: Cmd = Parser::try_parse_from([
            "ctl",
            "get",
            "claims",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
            "--js-domain",
            JS_DOMAIN,
        ])?;
        match get_claims_all.command {
            CtlCliCommand::Get(CtlGetCommand::Claims(GetClaimsCommand { opts })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(opts.js_domain.unwrap(), JS_DOMAIN);
            }
            cmd => panic!("ctl get claims constructed incorrect command {cmd:?}"),
        }
        let link_all: Cmd = Parser::try_parse_from([
            "ctl",
            "link",
            "put",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
            "--link-name",
            "default",
            ACTOR_ID,
            PROVIDER_ID,
            "wasmcloud:provider",
            "THING=foo",
        ])?;
        use wash_lib::cli::link::LinkPutCommand;
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
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(actor_id, ACTOR_ID.parse()?);
                assert_eq!(provider_id, PROVIDER_ID.parse()?);
                assert_eq!(contract_id, "wasmcloud:provider".to_string());
                assert_eq!(link_name.unwrap(), "default".to_string());
                assert_eq!(values, vec!["THING=foo".to_string()]);
            }
            cmd => panic!("ctl link put constructed incorrect command {cmd:?}"),
        }
        let update_all: Cmd = Parser::try_parse_from([
            "ctl",
            "update",
            "actor",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
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
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, HOST_ID.parse()?);
                assert_eq!(actor_id, ACTOR_ID.parse()?);
                assert_eq!(new_actor_ref, "wasmcloud.azurecr.io/actor:v2".to_string());
            }
            cmd => panic!("ctl get claims constructed incorrect command {cmd:?}"),
        }

        let scale_actor_all: Cmd = Parser::try_parse_from([
            "ctl",
            "scale",
            "actor",
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
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
            crate::CtlCliCommand::Scale(ScaleCommand::Actor(super::ScaleActorCommand {
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
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, HOST_ID.parse()?);
                assert_eq!(actor_id, ACTOR_ID.parse()?);
                assert_eq!(actor_ref, "wasmcloud.azurecr.io/actor:v2".to_string());
                assert_eq!(count, 1);
                assert_eq!(annotations, vec!["foo=bar".to_string()]);
            }
            cmd => panic!("ctl scale actor constructed incorrect command {cmd:?}"),
        }

        Ok(())
    }
}
