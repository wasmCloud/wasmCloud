use anyhow::Result;

use wash_lib::cli::stop::{handle_stop_actor, stop_host, stop_provider, StopCommand};

use crate::{appearance::spinner::Spinner, CommandOutput, OutputKind};

pub async fn handle_command(
    command: StopCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out: CommandOutput = match command {
        StopCommand::Actor(cmd) => {
            let actor_id = &cmd.actor_id.to_string();
            sp.update_spinner_message(format!(" Stopping actor {actor_id} ... "));
            handle_stop_actor(cmd).await?
        }
        StopCommand::Provider(cmd) => {
            let provider_id = &cmd.provider_id.to_string();
            sp.update_spinner_message(format!(" Stopping provider {provider_id} ... "));
            stop_provider(cmd).await?
        }
        StopCommand::Host(cmd) => {
            let host_id = &cmd.host_id.to_string();
            sp.update_spinner_message(format!(" Stopping host {host_id} ... "));
            stop_host(cmd).await?
        }
    };

    Ok(out)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::CtlCliCommand;
    use clap::Parser;
    use wash_lib::cli::stop::{StopActorCommand, StopHostCommand, StopProviderCommand};

    #[derive(Parser)]
    struct Cmd {
        #[clap(subcommand)]
        command: CtlCliCommand,
    }

    const CTL_HOST: &str = "127.0.0.1";
    const CTL_PORT: &str = "4222";
    const LATTICE_PREFIX: &str = "default";

    const ACTOR_ID: &str = "MDPDJEYIAK6MACO67PRFGOSSLODBISK4SCEYDY3HEOY4P5CVJN6UCWUK";
    const HOST_ID: &str = "NCE7YHGI42RWEKBRDJZWXBEJJCFNE5YIWYMSTLGHQBEGFY55BKJ3EG3G";
    const PROVIDER_ID: &str = "VBKTSBG2WKP6RJWLQ5O7RDVIIB4LMW6U5R67A7QMIDBZDGZWYTUE3TSI";
    const CONTRACT_ID: &str = "wasmcloud:httpserver";
    const CONTEXT_PATH: &str = "/tmp/fake/context";
    const CTL_JWT: &str = "not-a-jwt";
    const CTL_SEED: &str = "not-a-seed";
    const CTL_CREDSFILE: &str = "/tmp/fake/credsfile";
    const JS_DOMAIN: &str = "js";
    const TIMEOUT_MS: u64 = 2001;
    const HOST_TIMEOUT_MS: u64 = 3001;
    const LINK_NAME: &str = "default";

    #[test]
    /// Enumerates multiple options of the `stop actor` subcommand to ensure API doesn't
    /// change between versions. This test will fail if the subcommand
    /// changes syntax, ordering of required elements, or flags.
    fn test_stop_actor_cmd_comprehensive() -> Result<()> {
        let stop_actor_all: Cmd = Parser::try_parse_from([
            "ctl",
            "stop",
            "actor",
            HOST_ID,
            ACTOR_ID,
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--ctl-jwt",
            CTL_JWT,
            "--ctl-seed",
            CTL_SEED,
            "--ctl-credsfile",
            CTL_CREDSFILE,
            "--timeout-ms",
            &TIMEOUT_MS.to_string(),
            "--context",
            CONTEXT_PATH,
            "--js-domain",
            JS_DOMAIN,
            "--count",
            "1",
            "--skip-wait",
        ])?;

        match stop_actor_all.command {
            #[allow(deprecated)]
            CtlCliCommand::Stop(StopCommand::Actor(StopActorCommand {
                opts,
                host_id,
                actor_id,
                skip_wait,
                count,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert!(skip_wait);
                assert_eq!(count, 1);
                assert_eq!(host_id.to_string(), HOST_ID);
                assert_eq!(actor_id.to_string(), ACTOR_ID,);
            }
            cmd => panic!("stop actor constructed incorrect command {cmd:?}"),
        }

        Ok(())
    }

    #[test]
    /// Enumerates multiple options of the `stop actor` subcommand to ensure API doesn't
    /// change between versions. This test will fail if the subcommand
    /// changes syntax, ordering of required elements, or flags.
    fn test_stop_provider_cmd_comprehensive() -> Result<()> {
        let stop_provider_all: Cmd = Parser::try_parse_from([
            "ctl",
            "stop",
            "provider",
            HOST_ID,
            PROVIDER_ID,
            LINK_NAME,
            CONTRACT_ID,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--ctl-jwt",
            CTL_JWT,
            "--ctl-seed",
            CTL_SEED,
            "--ctl-credsfile",
            CTL_CREDSFILE,
            "--js-domain",
            JS_DOMAIN,
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--timeout-ms",
            &TIMEOUT_MS.to_string(),
            "--context",
            CONTEXT_PATH,
            "--skip-wait",
        ])?;
        match stop_provider_all.command {
            CtlCliCommand::Stop(StopCommand::Provider(StopProviderCommand {
                opts,
                host_id,
                provider_id,
                link_name,
                skip_wait,
                contract_id,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(link_name, "default".to_string());
                assert_eq!(host_id.to_string(), HOST_ID);
                assert_eq!(contract_id, CONTRACT_ID);
                assert_eq!(link_name, LINK_NAME);
                assert_eq!(provider_id.to_string(), PROVIDER_ID,);
                assert!(skip_wait);
            }
            cmd => panic!("stop provider constructed incorrect command {cmd:?}"),
        }

        Ok(())
    }

    #[test]
    /// Enumerates multiple options of the `stop host` subcommand to ensure API doesn't
    /// change between versions. This test will fail if the subcommand
    /// changes syntax, ordering of required elements, or flags.
    fn test_stop_host_cmd_comprehensive() -> Result<()> {
        let stop_host_all: Cmd = Parser::try_parse_from([
            "ctl",
            "stop",
            "host",
            HOST_ID,
            "--ctl-host",
            CTL_HOST,
            "--ctl-port",
            CTL_PORT,
            "--ctl-jwt",
            CTL_JWT,
            "--ctl-seed",
            CTL_SEED,
            "--ctl-credsfile",
            CTL_CREDSFILE,
            "--js-domain",
            JS_DOMAIN,
            "--lattice-prefix",
            LATTICE_PREFIX,
            "--timeout-ms",
            &TIMEOUT_MS.to_string(),
            "--context",
            CONTEXT_PATH,
            "--host-timeout",
            &HOST_TIMEOUT_MS.to_string(),
        ])?;
        match stop_host_all.command {
            CtlCliCommand::Stop(StopCommand::Host(StopHostCommand {
                opts,
                host_id,
                host_shutdown_timeout,
                ..
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice_prefix.unwrap(), LATTICE_PREFIX);
                assert_eq!(opts.timeout_ms, TIMEOUT_MS);
                assert_eq!(host_shutdown_timeout, HOST_TIMEOUT_MS);
                assert_eq!(host_id.to_string(), HOST_ID);
                assert_eq!(host_id.to_string(), HOST_ID,);
            }
            cmd => panic!("stop host constructed incorrect command {cmd:?}"),
        }

        Ok(())
    }
}
