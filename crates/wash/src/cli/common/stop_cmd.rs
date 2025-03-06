use anyhow::Result;

use crate::lib::cli::stop::{handle_stop_component, handle_stop_provider, stop_host, StopCommand};
use crate::lib::cli::{CommandOutput, OutputKind};

use crate::appearance::spinner::Spinner;

pub async fn handle_command(
    command: StopCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out: CommandOutput = match command {
        StopCommand::Component(cmd) => {
            let component_id = &cmd.component_id.to_string();
            sp.update_spinner_message(format!(" Stopping component {component_id} ... "));
            handle_stop_component(cmd).await?
        }
        StopCommand::Provider(cmd) => {
            let provider_id = &cmd.provider_id.to_string();
            sp.update_spinner_message(format!(" Stopping provider {provider_id} ... "));
            handle_stop_provider(cmd).await?
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
    use crate::ctl::CtlCliCommand;

    use super::*;
    use clap::Parser;
    use crate::lib::cli::stop::{StopComponentCommand, StopHostCommand, StopProviderCommand};

    #[derive(Parser)]
    struct Cmd {
        #[clap(subcommand)]
        command: CtlCliCommand,
    }

    const CTL_HOST: &str = "127.0.0.1";
    const CTL_PORT: &str = "4222";
    const DEFAULT_LATTICE: &str = "default";

    const COMPONENT_ID: &str = "MDPDJEYIAK6MACO67PRFGOSSLODBISK4SCEYDY3HEOY4P5CVJN6UCWUK";
    const HOST_ID: &str = "NCE7YHGI42RWEKBRDJZWXBEJJCFNE5YIWYMSTLGHQBEGFY55BKJ3EG3G";
    const PROVIDER_ID: &str = "VBKTSBG2WKP6RJWLQ5O7RDVIIB4LMW6U5R67A7QMIDBZDGZWYTUE3TSI";
    const CONTEXT_PATH: &str = "/tmp/fake/context";
    const CTL_JWT: &str = "not-a-jwt";
    const CTL_SEED: &str = "not-a-seed";
    const CTL_CREDSFILE: &str = "/tmp/fake/credsfile";
    const JS_DOMAIN: &str = "js";
    const TIMEOUT_MS: u64 = 2001;
    const HOST_TIMEOUT_MS: u64 = 3001;

    #[test]
    /// Enumerates multiple options of the `stop component` subcommand to ensure API doesn't
    /// change between versions. This test will fail if the subcommand
    /// changes syntax, ordering of required elements, or flags.
    fn test_stop_component_cmd_comprehensive() -> Result<()> {
        let stop_component_all: Cmd = Parser::try_parse_from([
            "ctl",
            "stop",
            "component",
            "--host-id",
            HOST_ID,
            COMPONENT_ID,
            "--lattice",
            DEFAULT_LATTICE,
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
            "--skip-wait",
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
                assert!(skip_wait);
                assert_eq!(host_id.unwrap(), HOST_ID);
                assert_eq!(component_id, COMPONENT_ID);
            }
            cmd => panic!("stop component constructed incorrect command {cmd:?}"),
        }

        Ok(())
    }

    #[test]
    /// Enumerates multiple options of the `stop component` subcommand to ensure API doesn't
    /// change between versions. This test will fail if the subcommand
    /// changes syntax, ordering of required elements, or flags.
    fn test_stop_provider_cmd_comprehensive() -> Result<()> {
        let stop_provider_all: Cmd = Parser::try_parse_from([
            "ctl",
            "stop",
            "provider",
            "--host-id",
            HOST_ID,
            PROVIDER_ID,
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
            "--lattice",
            DEFAULT_LATTICE,
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
                skip_wait,
            })) => {
                assert_eq!(&opts.ctl_host.unwrap(), CTL_HOST);
                assert_eq!(&opts.ctl_port.unwrap(), CTL_PORT);
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, 2001);
                assert_eq!(host_id.unwrap(), HOST_ID);
                assert_eq!(provider_id, PROVIDER_ID);
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
            "--lattice",
            DEFAULT_LATTICE,
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
                assert_eq!(&opts.lattice.unwrap(), DEFAULT_LATTICE);
                assert_eq!(opts.timeout_ms, TIMEOUT_MS);
                assert_eq!(host_shutdown_timeout, HOST_TIMEOUT_MS);
                assert_eq!(host_id, HOST_ID);
                assert_eq!(host_id, HOST_ID,);
            }
            cmd => panic!("stop host constructed incorrect command {cmd:?}"),
        }

        Ok(())
    }
}
