use clap::Subcommand;

pub use output::*;
use wash_lib::cli::{
    get::{GetClaimsCommand, GetHostInventoriesCommand, GetHostsCommand},
    link::LinkCommand,
    scale::ScaleCommand,
    start::StartCommand,
    stop::StopCommand,
    update::UpdateCommand,
};

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

#[cfg(test)]
mod test {
    use clap::Parser;

    use wash_lib::cli::{
        get::GetHostsCommand,
        scale::ScaleActorCommand,
        stop::{StopActorCommand, StopProviderCommand},
        update::UpdateActorCommand,
    };

    use super::*;

    #[derive(Parser)]
    struct Cmd {
        #[clap(subcommand)]
        command: CtlCliCommand,
    }

    const CTL_HOST: &str = "127.0.0.1";
    const CTL_PORT: &str = "4222";
    const DEFAULT_LATTICE: &str = "default";
    const JS_DOMAIN: &str = "custom-domain";

    const ACTOR_ID: &str = "MDPDJEYIAK6MACO67PRFGOSSLODBISK4SCEYDY3HEOY4P5CVJN6UCWUK";
    const PROVIDER_ID: &str = "VBKTSBG2WKP6RJWLQ5O7RDVIIB4LMW6U5R67A7QMIDBZDGZWYTUE3TSI";
    const HOST_ID: &str = "NCE7YHGI42RWEKBRDJZWXBEJJCFNE5YIWYMSTLGHQBEGFY55BKJ3EG3G";

    #[test]
    /// Enumerates multiple options of the `ctl` command to ensure API doesn't
    /// change between versions. This test will fail if any subcommand of `wash ctl`
    /// changes syntax, ordering of required elements, or flags.
    fn test_ctl_comprehensive() -> anyhow::Result<()> {
        let stop_actor_all: Cmd = Parser::try_parse_from([
            "ctl",
            "stop",
            "actor",
            "--lattice",
            DEFAULT_LATTICE,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
            "--host-id",
            HOST_ID,
            ACTOR_ID,
        ])?;
        match stop_actor_all.command {
            CtlCliCommand::Stop(StopCommand::Actor(StopActorCommand {
                opts,
                host_id,
                actor_id,
                skip_wait,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, Some(HOST_ID.to_string()));
                assert_eq!(actor_id, ACTOR_ID);
                assert!(!skip_wait);
            }
            cmd => panic!("ctl stop actor constructed incorrect command {cmd:?}"),
        }
        let stop_actor_minimal: Cmd = Parser::try_parse_from(["ctl", "stop", "actor", "foobar"])?;
        match stop_actor_minimal.command {
            CtlCliCommand::Stop(StopCommand::Actor(StopActorCommand {
                host_id, actor_id, ..
            })) => {
                assert_eq!(host_id, None);
                assert_eq!(actor_id, "foobar");
            }
            cmd => panic!("ctl stop actor constructed incorrect command {cmd:?}"),
        }
        let stop_provider_all: Cmd = Parser::try_parse_from([
            "ctl",
            "stop",
            "provider",
            "--lattice",
            DEFAULT_LATTICE,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
            "--host-id",
            HOST_ID,
            PROVIDER_ID,
            "wasmcloud:provider",
            "blahblah",
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
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, Some(HOST_ID.to_string()));
                assert_eq!(provider_id, PROVIDER_ID);
                assert_eq!(link_name, "blahblah");
                assert_eq!(contract_id, "wasmcloud:provider".to_string());
                assert!(!skip_wait);
            }
            cmd => panic!("ctl stop actor constructed incorrect command {cmd:?}"),
        }
        let stop_provider_minimal: Cmd =
            Parser::try_parse_from(["ctl", "stop", "provider", "foobar", "wasmcloud:provider"])?;
        match stop_provider_minimal.command {
            CtlCliCommand::Stop(StopCommand::Provider(StopProviderCommand {
                host_id,
                provider_id,
                link_name,
                contract_id,
                ..
            })) => {
                assert_eq!(host_id, None);
                assert_eq!(provider_id, "foobar");
                assert_eq!(link_name, "default");
                assert_eq!(contract_id, "wasmcloud:provider");
            }
            cmd => panic!("ctl stop actor constructed incorrect command {cmd:?}"),
        }
        let get_hosts_all: Cmd = Parser::try_parse_from([
            "ctl",
            "get",
            "hosts",
            "--lattice",
            DEFAULT_LATTICE,
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
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
            }
            cmd => panic!("ctl get hosts constructed incorrect command {cmd:?}"),
        }
        let get_host_inventory_all: Cmd = Parser::try_parse_from([
            "ctl",
            "get",
            "inventory",
            "--lattice",
            DEFAULT_LATTICE,
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
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id.unwrap(), HOST_ID.parse()?);
            }
            cmd => panic!("ctl get inventory constructed incorrect command {cmd:?}"),
        }
        let get_claims_all: Cmd = Parser::try_parse_from([
            "ctl",
            "get",
            "claims",
            "--lattice",
            DEFAULT_LATTICE,
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
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(opts.js_domain.unwrap(), JS_DOMAIN);
            }
            cmd => panic!("ctl get claims constructed incorrect command {cmd:?}"),
        }
        let link_all: Cmd = Parser::try_parse_from([
            "ctl",
            "link",
            "put",
            "--lattice",
            DEFAULT_LATTICE,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
            "--link-name",
            "notdefault",
            ACTOR_ID,
            PROVIDER_ID,
            "wasmcloud",
            "provider",
            "--interface",
            "foo",
        ])?;
        use wash_lib::cli::link::LinkPutCommand;
        match link_all.command {
            CtlCliCommand::Link(LinkCommand::Put(LinkPutCommand {
                opts,
                source_id,
                target,
                wit_namespace,
                wit_package,
                interfaces,
                source_config,
                target_config,
                link_name,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(source_id, ACTOR_ID);
                assert_eq!(target, PROVIDER_ID);
                assert_eq!(wit_namespace, "wasmcloud".to_string());
                assert_eq!(wit_package, "provider".to_string());
                assert_eq!(link_name.unwrap(), "notdefault".to_string());
                assert_eq!(interfaces.as_slice(), &["foo".to_string()]);
                assert!(source_config.is_empty());
                assert!(target_config.is_empty());
            }
            cmd => panic!("ctl link put constructed incorrect command {cmd:?}"),
        }
        let update_all: Cmd = Parser::try_parse_from([
            "ctl",
            "update",
            "actor",
            "--lattice",
            DEFAULT_LATTICE,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
            "--host-id",
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
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, Some(HOST_ID.to_string()));
                assert_eq!(actor_id, ACTOR_ID);
                assert_eq!(new_actor_ref, "wasmcloud.azurecr.io/actor:v2".to_string());
            }
            cmd => panic!("ctl get claims constructed incorrect command {cmd:?}"),
        }

        let scale_actor_all: Cmd = Parser::try_parse_from([
            "ctl",
            "scale",
            "actor",
            "--lattice",
            DEFAULT_LATTICE,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
            HOST_ID,
            "wasmcloud.azurecr.io/actor:v2",
            "myactorv2",
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
                actor_id,
                max_instances,
                annotations,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, HOST_ID);
                assert_eq!(actor_ref, "wasmcloud.azurecr.io/actor:v2".to_string());
                assert_eq!(actor_id, "myactorv2".to_string());
                assert_eq!(max_instances, 1);
                assert_eq!(annotations, vec!["foo=bar".to_string()]);
            }
            cmd => panic!("ctl scale actor constructed incorrect command {cmd:?}"),
        }

        Ok(())
    }
}
