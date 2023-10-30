use crate::appearance::spinner::Spinner;

use anyhow::Result;
use wash_lib::cli::start::{handle_start_actor, start_provider, StartCommand};
use wash_lib::cli::{CommandOutput, OutputKind};

pub async fn handle_command(
    command: StartCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out: CommandOutput = match command {
        StartCommand::Actor(cmd) => {
            let actor_ref = &cmd.actor_ref.to_string();

            sp.update_spinner_message(format!(" Starting actor {actor_ref} ... "));

            handle_start_actor(cmd).await?
        }
        StartCommand::Provider(cmd) => {
            let provider_ref = &cmd.provider_ref.to_string();

            sp.update_spinner_message(format!(" Starting provider {provider_ref} ... "));

            start_provider(cmd).await?
        }
    };

    Ok(out)
}

#[cfg(test)]
mod test {
    use super::*;

    use crate::ctl::CtlCliCommand;

    use clap::Parser;
    use wash_lib::cli::start::{StartActorCommand, StartProviderCommand};

    #[derive(Parser)]
    struct Cmd {
        #[clap(subcommand)]
        command: CtlCliCommand,
    }

    const CTL_HOST: &str = "127.0.0.1";
    const CTL_PORT: &str = "4222";
    const LATTICE_PREFIX: &str = "default";

    const HOST_ID: &str = "NCE7YHGI42RWEKBRDJZWXBEJJCFNE5YIWYMSTLGHQBEGFY55BKJ3EG3G";

    #[test]
    /// Enumerates multiple options of the `ctl` command to ensure API doesn't
    /// change between versions. This test will fail if any subcommand of `wash ctl`
    /// changes syntax, ordering of required elements, or flags.
    fn test_ctl_comprehensive() -> Result<()> {
        let start_actor_all: Cmd = Parser::try_parse_from([
            "ctl",
            "start",
            "actor",
            "--lattice-prefix",
            LATTICE_PREFIX,
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
        ])?;
        match start_actor_all.command {
            CtlCliCommand::Start(StartCommand::Actor(StartActorCommand {
                opts,
                host_id,
                actor_ref,
                constraints,
                auction_timeout_ms,
                ..
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(auction_timeout_ms, 2002);
                assert_eq!(host_id.unwrap(), HOST_ID.parse()?);
                assert_eq!(actor_ref, "wasmcloud.azurecr.io/actor:v1".to_string());
                assert_eq!(constraints.unwrap(), vec!["arch=x86_64".to_string()]);
            }
            cmd => panic!("ctl start actor constructed incorrect command {cmd:?}"),
        }
        let start_provider_all: Cmd = Parser::try_parse_from([
            "ctl",
            "start",
            "provider",
            "--lattice-prefix",
            LATTICE_PREFIX,
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
        ])?;
        match start_provider_all.command {
            CtlCliCommand::Start(StartCommand::Provider(StartProviderCommand {
                opts,
                host_id,
                provider_ref,
                link_name,
                constraints,
                auction_timeout_ms,
                config_json,
                skip_wait,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(config_json, None);
                assert_eq!(auction_timeout_ms, 2002);
                assert_eq!(link_name, "default".to_string());
                assert_eq!(constraints.unwrap(), vec!["arch=x86_64".to_string()]);
                assert_eq!(host_id.unwrap(), HOST_ID.parse()?);
                assert_eq!(provider_ref, "wasmcloud.azurecr.io/provider:v1".to_string());
                assert!(skip_wait);
            }
            cmd => panic!("ctl start provider constructed incorrect command {cmd:?}"),
        }
        Ok(())
    }
}
