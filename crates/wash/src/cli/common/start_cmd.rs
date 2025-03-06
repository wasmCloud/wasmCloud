use crate::appearance::spinner::Spinner;

use anyhow::Result;
use crate::lib::cli::start::{handle_start_component, handle_start_provider, StartCommand};
use crate::lib::cli::{CommandOutput, OutputKind};

pub async fn handle_command(
    command: StartCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out: CommandOutput = match command {
        StartCommand::Component(cmd) => {
            let component_ref = &cmd.component_ref.to_string();

            sp.update_spinner_message(format!(" Starting component {component_ref} ... "));

            handle_start_component(cmd).await?
        }
        StartCommand::Provider(cmd) => {
            let provider_ref = &cmd.provider_ref.to_string();

            sp.update_spinner_message(format!(" Starting provider {provider_ref} ... "));

            handle_start_provider(cmd).await?
        }
    };

    Ok(out)
}

#[cfg(test)]
mod test {
    use super::*;

    use crate::ctl::CtlCliCommand;

    use clap::Parser;
    use crate::lib::cli::start::{StartComponentCommand, StartProviderCommand};

    #[derive(Parser)]
    struct Cmd {
        #[clap(subcommand)]
        command: CtlCliCommand,
    }

    const CTL_HOST: &str = "127.0.0.1";
    const CTL_PORT: &str = "4222";
    const DEFAULT_LATTICE: &str = "default";

    const HOST_ID: &str = "NCE7YHGI42RWEKBRDJZWXBEJJCFNE5YIWYMSTLGHQBEGFY55BKJ3EG3G";

    #[test]
    /// Enumerates multiple options of the `ctl` command to ensure API doesn't
    /// change between versions. This test will fail if any subcommand of `wash ctl`
    /// changes syntax, ordering of required elements, or flags.
    fn test_ctl_comprehensive() -> Result<()> {
        let start_component_all: Cmd = Parser::try_parse_from([
            "ctl",
            "start",
            "component",
            "--lattice",
            DEFAULT_LATTICE,
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
            "wasmcloud.azurecr.io/component:v1",
            "mycomponent",
        ])?;
        match start_component_all.command {
            CtlCliCommand::Start(StartCommand::Component(StartComponentCommand {
                opts,
                host_id,
                component_ref,
                component_id,
                constraints,
                auction_timeout_ms,
                ..
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(auction_timeout_ms, 2002);
                assert_eq!(host_id.unwrap(), HOST_ID.to_string());
                assert_eq!(
                    component_ref,
                    "wasmcloud.azurecr.io/component:v1".to_string()
                );
                assert_eq!(component_id, "mycomponent".to_string());
                assert_eq!(constraints.unwrap(), vec!["arch=x86_64".to_string()]);
            }
            cmd => panic!("ctl start component constructed incorrect command {cmd:?}"),
        }
        let start_provider_all: Cmd = Parser::try_parse_from([
            "ctl",
            "start",
            "provider",
            "--lattice",
            DEFAULT_LATTICE,
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
            "--link-name",
            "default",
            "--skip-wait",
            "wasmcloud.azurecr.io/provider:v1",
            "providerv1",
        ])?;
        match start_provider_all.command {
            CtlCliCommand::Start(StartCommand::Provider(StartProviderCommand {
                opts,
                host_id,
                provider_ref,
                provider_id,
                link_name,
                constraints,
                auction_timeout_ms,
                config,
                skip_wait,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(auction_timeout_ms, 2002);
                assert_eq!(link_name, "default".to_string());
                assert_eq!(constraints.unwrap(), vec!["arch=x86_64".to_string()]);
                assert_eq!(host_id.unwrap(), HOST_ID.to_string());
                assert_eq!(provider_ref, "wasmcloud.azurecr.io/provider:v1".to_string());
                assert_eq!(provider_id, "providerv1".to_string());
                assert!(config.is_empty());
                assert!(skip_wait);
            }
            cmd => panic!("ctl start provider constructed incorrect command {cmd:?}"),
        }
        Ok(())
    }
}
