use clap::Subcommand;

pub use output::*;
use crate::lib::cli::{
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

    /// Link an component and a provider
    #[clap(name = "link", alias = "links", subcommand)]
    Link(LinkCommand),

    /// Start an component or a provider
    #[clap(name = "start", subcommand)]
    Start(StartCommand),

    /// Stop an component, provider, or host
    #[clap(name = "stop", subcommand)]
    Stop(StopCommand),

    /// Update an component running in a host to a new component
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

    /// Query a single host for its inventory of labels, components and providers
    #[clap(name = "inventory")]
    HostInventories(GetHostInventoriesCommand),

    /// Query lattice for its claims cache
    #[clap(name = "claims")]
    Claims(GetClaimsCommand),
}

#[cfg(test)]
mod test {
    use clap::Parser;

    use crate::lib::cli::{
        get::GetHostsCommand,
        scale::ScaleComponentCommand,
        stop::{StopComponentCommand, StopProviderCommand},
        update::UpdateComponentCommand,
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

    const COMPONENT_ID: &str = "MDPDJEYIAK6MACO67PRFGOSSLODBISK4SCEYDY3HEOY4P5CVJN6UCWUK";
    const PROVIDER_ID: &str = "VBKTSBG2WKP6RJWLQ5O7RDVIIB4LMW6U5R67A7QMIDBZDGZWYTUE3TSI";
    const HOST_ID: &str = "NCE7YHGI42RWEKBRDJZWXBEJJCFNE5YIWYMSTLGHQBEGFY55BKJ3EG3G";

    #[test]
    /// Enumerates multiple options of the `ctl` command to ensure API doesn't
    /// change between versions. This test will fail if any subcommand of `wash ctl`
    /// changes syntax, ordering of required elements, or flags.
    fn test_ctl_comprehensive() -> anyhow::Result<()> {
        let stop_component_all: Cmd = Parser::try_parse_from([
            "ctl",
            "stop",
            "component",
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
            COMPONENT_ID,
        ])?;
        match stop_component_all.command {
            CtlCliCommand::Stop(StopCommand::Component(StopComponentCommand {
                opts,
                host_id,
                component_id,
                skip_wait,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, Some(HOST_ID.to_string()));
                assert_eq!(component_id, COMPONENT_ID);
                assert!(!skip_wait);
            }
            cmd => panic!("ctl stop component constructed incorrect command {cmd:?}"),
        }
        let stop_component_minimal: Cmd =
            Parser::try_parse_from(["ctl", "stop", "component", "foobar"])?;
        match stop_component_minimal.command {
            CtlCliCommand::Stop(StopCommand::Component(StopComponentCommand {
                host_id,
                component_id,
                ..
            })) => {
                assert_eq!(host_id, None);
                assert_eq!(component_id, "foobar");
            }
            cmd => panic!("ctl stop component constructed incorrect command {cmd:?}"),
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
        ])?;
        match stop_provider_all.command {
            CtlCliCommand::Stop(StopCommand::Provider(StopProviderCommand {
                opts,
                host_id,
                provider_id,
                skip_wait,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, Some(HOST_ID.to_string()));
                assert_eq!(provider_id, PROVIDER_ID);
                assert!(!skip_wait);
            }
            cmd => panic!("ctl stop component constructed incorrect command {cmd:?}"),
        }
        let stop_provider_minimal: Cmd =
            Parser::try_parse_from(["ctl", "stop", "provider", "foobar"])?;
        match stop_provider_minimal.command {
            CtlCliCommand::Stop(StopCommand::Provider(StopProviderCommand {
                host_id,
                provider_id,
                ..
            })) => {
                assert_eq!(host_id, None);
                assert_eq!(provider_id, "foobar");
            }
            cmd => panic!("ctl stop component constructed incorrect command {cmd:?}"),
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
                watch: _,
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
            COMPONENT_ID,
            PROVIDER_ID,
            "wasmcloud",
            "provider",
            "--interface",
            "foo",
        ])?;
        use crate::lib::cli::link::LinkPutCommand;
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
                assert_eq!(source_id, COMPONENT_ID);
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
            "component",
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
            COMPONENT_ID,
            "wasmcloud.azurecr.io/component:v2",
        ])?;
        match update_all.command {
            CtlCliCommand::Update(UpdateCommand::Component(UpdateComponentCommand {
                opts,
                host_id,
                component_id,
                new_component_ref,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, Some(HOST_ID.to_string()));
                assert_eq!(component_id, COMPONENT_ID);
                assert_eq!(
                    new_component_ref,
                    "wasmcloud.azurecr.io/component:v2".to_string()
                );
            }
            cmd => panic!("ctl get claims constructed incorrect command {cmd:?}"),
        }

        let scale_component_all: Cmd = Parser::try_parse_from([
            "ctl",
            "scale",
            "component",
            "--lattice",
            DEFAULT_LATTICE,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--timeout-ms",
            "2001",
            HOST_ID,
            "wasmcloud.azurecr.io/component:v2",
            "mycomponentv2",
            "--count",
            "1",
            "--annotations",
            "foo=bar",
            "--config",
            "default-port",
            "--config",
            "lang",
        ])?;

        match scale_component_all.command {
            CtlCliCommand::Scale(ScaleCommand::Component(ScaleComponentCommand {
                opts,
                host_id,
                component_ref,
                component_id,
                max_instances,
                annotations,
                config,
                skip_wait,
                wait_timeout_ms,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id, HOST_ID);
                assert_eq!(
                    component_ref,
                    "wasmcloud.azurecr.io/component:v2".to_string()
                );
                assert_eq!(component_id, "mycomponentv2".to_string());
                assert_eq!(max_instances, 1);
                assert_eq!(annotations, vec!["foo=bar".to_string()]);
                assert_eq!(config, vec!["default-port", "lang"]);
                assert!(!skip_wait);
                assert_eq!(wait_timeout_ms, 5000);
            }
            cmd => panic!("ctl scale component constructed incorrect command {cmd:?}"),
        }

        Ok(())
    }
}
