use anyhow::Result;
use clap::Subcommand;
use wash_lib::cli::{
    get::{GetClaimsCommand, GetCommand, GetHostInventoriesCommand, GetHostsCommand},
    link::LinkCommand,
    scale::{handle_scale_actor, ScaleCommand},
    start::StartCommand,
    stop::{handle_stop_actor, stop_host, stop_provider, StopCommand},
    update::{handle_update_actor, UpdateCommand},
    CommandOutput, OutputKind,
};

use crate::{
    appearance::spinner::Spinner,
    common::link_cmd::handle_command as handle_link_command,
    common::{
        get_cmd::handle_command as handle_get_command,
        start_cmd::handle_command as handle_start_command,
    },
};
pub use output::*;

mod output;

#[derive(Debug, Clone, Subcommand)]
pub enum CtlCliCommand {
    /// Retrieves information about the lattice
    #[clap(name = "get", subcommand)]
    Get(CtlGetCommand),

    /// Link an actor and a provider
    #[clap(name = "link", alias = "links", subcommand)]
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

    #[clap(name = "scale", subcommand)]
    Scale(ScaleCommand),
}

#[derive(Debug, Clone, Subcommand)]
pub enum CtlGetCommand {
    /// Query lattice for running hosts
    #[clap(name = "hosts")]
    Hosts(GetHostsCommand),

    /// Query a single host for its inventory of labels, actors and providers
    #[clap(name = "inventory")]
    HostInventories(GetHostInventoriesCommand),

    /// Query lattice for its claims cache
    #[clap(name = "claims")]
    Claims(GetClaimsCommand),
}

pub async fn handle_command(
    command: CtlCliCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    use CtlCliCommand::*;
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out: CommandOutput = match command {
        Get(CtlGetCommand::Hosts(cmd)) => {
            eprintln!("[warn] `wash ctl get hosts` has been deprecated in favor of `wash get hosts` and will be removed in a future version.");
            handle_get_command(GetCommand::Hosts(cmd), output_kind).await?
        }
        Get(CtlGetCommand::HostInventories(cmd)) => {
            eprintln!("[warn] `wash ctl get inventory` has been deprecated in favor of `wash get inventory` and will be removed in a future version.");
            handle_get_command(GetCommand::HostInventories(cmd), output_kind).await?
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
            eprintln!("[warn] `wash ctl stop` has been deprecated in favor of `wash stop` and will be removed in a future version.");
            sp.update_spinner_message(format!(" Stopping provider {} ... ", cmd.provider_id));

            stop_provider(cmd.clone()).await?
        }
        Stop(StopCommand::Host(cmd)) => {
            eprintln!("[warn] `wash ctl stop` has been deprecated in favor of `wash stop` and will be removed in a future version.");
            sp.update_spinner_message(format!(" Stopping host {} ... ", cmd.host_id));

            stop_host(cmd.clone()).await?
        }
        Update(UpdateCommand::Actor(cmd)) => {
            eprintln!("[warn] `wash ctl update actor` has been deprecated in favor of `wash update actor` and will be removed in a future version.");
            sp.update_spinner_message(format!(
                " Updating Actor {} to {} ... ",
                cmd.actor_id, cmd.new_actor_ref
            ));

            handle_update_actor(cmd.clone()).await?
        }
        Scale(ScaleCommand::Actor(cmd)) => {
            eprintln!("[warn] `wash ctl scale actor` has been deprecated in favor of `wash scale actor` and will be removed in a future version.");
            let max = cmd
                .max_concurrent
                .map(|max| max.to_string())
                .unwrap_or_else(|| "unbounded".to_string());
            sp.update_spinner_message(format!(
                " Scaling Actor {} to {} max concurrent instances ... ",
                cmd.actor_ref, max
            ));
            handle_scale_actor(cmd.clone()).await?
        }
    };

    sp.finish_and_clear();

    Ok(out)
}

#[cfg(test)]
mod test {
    use super::*;

    use clap::Parser;
    use wash_lib::cli::{
        get::GetHostsCommand,
        scale::ScaleActorCommand,
        stop::{StopActorCommand, StopProviderCommand},
        update::UpdateActorCommand,
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
            #[allow(deprecated)]
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
            CtlCliCommand::Get(CtlGetCommand::HostInventories(GetHostInventoriesCommand {
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
            CtlCliCommand::Update(UpdateCommand::Actor(UpdateActorCommand {
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
            "wasmcloud.azurecr.io/actor:v2",
            "--count",
            "1",
            "--annotations",
            "foo=bar",
        ])?;

        match scale_actor_all.command {
            CtlCliCommand::Scale(ScaleCommand::Actor(ScaleActorCommand {
                opts,
                host_id,
                actor_ref,
                max_concurrent,
                annotations,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, HOST_ID.parse()?);
                assert_eq!(actor_ref, "wasmcloud.azurecr.io/actor:v2".to_string());
                assert_eq!(max_concurrent, Some(1));
                assert_eq!(annotations, vec!["foo=bar".to_string()]);
            }
            cmd => panic!("ctl scale actor constructed incorrect command {cmd:?}"),
        }

        Ok(())
    }
}
