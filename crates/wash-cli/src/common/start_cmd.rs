use crate::appearance::spinner::Spinner;

use anyhow::Result;
use wash_lib::cli::start::{handle_start, StartCommand};
use wash_lib::cli::{CommandOutput, OutputKind};

pub async fn handle_command(
    command: StartCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out: CommandOutput = {
        let component_ref = &command.component_ref.to_string();

        sp.update_spinner_message(format!(" Starting component {component_ref} ... "));

        handle_start(command).await?
    };

    Ok(out)
}

#[cfg(test)]
mod test {
    use super::*;

    use clap::Parser;
    use wash_lib::cli::start::StartCommand;

    #[derive(Debug, Parser)]
    struct Cmd {
        #[clap(flatten)]
        command: StartCommand,
    }

    const CTL_HOST: &str = "127.0.0.1";
    const CTL_PORT: &str = "4222";
    const DEFAULT_LATTICE: &str = "default";

    const HOST_ID: &str = "NCE7YHGI42RWEKBRDJZWXBEJJCFNE5YIWYMSTLGHQBEGFY55BKJ3EG3G";

    #[test]
    /// Enumerates multiple options of the `wash start` command to ensure API doesn't
    /// change between versions. This test will fail if any subcommand of `wash start`
    /// changes syntax, ordering of required elements, or flags.
    fn test_start_comprehensive() -> Result<()> {
        let start_component_all: Cmd = Parser::try_parse_from([
            "start",
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
            "wasmcloud.azurecr.io/actor:v1",
            "myactor",
        ])?;

        let StartCommand {
            opts,
            host_id,
            component_ref,
            component_id,
            constraints,
            auction_timeout_ms,
            ..
        } = start_component_all.command;

        assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
        assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
        assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
        assert_eq!(auction_timeout_ms, 2002);
        assert_eq!(host_id.unwrap(), HOST_ID.to_string());
        assert_eq!(component_ref, "wasmcloud.azurecr.io/actor:v1".to_string());
        assert_eq!(component_id, "myactor".to_string());
        assert_eq!(constraints.unwrap(), vec!["arch=x86_64".to_string()]);

        let start_provider_all: Cmd = Parser::try_parse_from([
            "start",
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

        let StartCommand {
            opts,
            host_id,
            component_ref,
            component_id,
            link_name,
            constraints,
            auction_timeout_ms,
            config,
            skip_wait,
            ..
        } = start_provider_all.command;

        assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
        assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
        assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
        assert_eq!(opts.timeout_ms, 2001);
        assert_eq!(auction_timeout_ms, 2002);
        assert_eq!(link_name, "default".to_string());
        assert_eq!(constraints.unwrap(), vec!["arch=x86_64".to_string()]);
        assert_eq!(host_id.unwrap(), HOST_ID.to_string());
        assert_eq!(
            component_ref,
            "wasmcloud.azurecr.io/provider:v1".to_string()
        );
        assert_eq!(component_id, "providerv1".to_string());
        assert!(config.is_empty());
        assert!(skip_wait);

        Ok(())
    }
}
