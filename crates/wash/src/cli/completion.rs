//! CLI command for generating shell completions

use crate::cli::{CliCommand, CliContext, CommandOutput};
use clap::Args;
use clap_complete::Shell;

#[derive(Debug, Clone, Args)]
pub struct CompletionCommand {
    /// The shell to generate completions for
    #[arg(value_enum)]
    shell: Shell,
}

impl CliCommand for CompletionCommand {
    async fn handle(&self, _ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        // The completion generation will be handled in main.rs where we have access to the full CLI
        // We'll just return the shell type that was requested
        Ok(CommandOutput::ok(
            format!("Generating completions for {}", self.shell),
            Some(serde_json::json!({"shell": format!("{}", self.shell)})),
        ))
    }
}

impl CompletionCommand {
    pub fn shell(&self) -> Shell {
        self.shell
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::cli::{OutputKind, test_support::context};
    use anyhow::{Context as _, Result};
    use clap::{Parser, ValueEnum, error::ErrorKind};
    use serde_json::json;

    #[derive(Parser)]
    struct CompletionCli {
        #[command(flatten)]
        command: CompletionCommand,
    }

    #[tokio::test]
    async fn every_shell_round_trips_through_arguments_getter_and_output() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let ctx = context(temp.path());
        let shells = [
            ("bash", Shell::Bash),
            ("elvish", Shell::Elvish),
            ("fish", Shell::Fish),
            ("powershell", Shell::PowerShell),
            ("zsh", Shell::Zsh),
        ];
        assert_eq!(Shell::value_variants().len(), shells.len());
        for (name, shell) in shells {
            let command = CompletionCli::try_parse_from(["completion", name])?.command;
            assert_eq!(command.shell(), shell);
            let output = command.handle(&ctx).await?;
            let message = format!("Generating completions for {name}");
            assert!(output.is_success());
            assert_eq!(output.text(), message);
            assert_eq!(output.json(), Some(&json!({"shell": name})));
            assert_eq!(
                output.clone().with_output_kind(OutputKind::Text).render(),
                (message.clone(), true)
            );
            let (rendered, success) = output.with_output_kind(OutputKind::Json).render();
            assert!(success);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&rendered)?,
                json!({
                    "message": message, "success": true, "data": {"shell": name},
                })
            );
        }
        assert_eq!(std::fs::read_dir(temp.path())?.count(), 0);
        Ok(())
    }

    #[test]
    fn cli_rejects_missing_unsupported_and_extra_shell_arguments() -> Result<()> {
        for (args, kind) in [
            (vec!["completion"], ErrorKind::MissingRequiredArgument),
            (vec!["completion", ""], ErrorKind::InvalidValue),
            (vec!["completion", "sh"], ErrorKind::InvalidValue),
            (vec!["completion", "BASH"], ErrorKind::InvalidValue),
            (
                vec!["completion", "bash", "zsh"],
                ErrorKind::UnknownArgument,
            ),
            (
                vec!["completion", "--shell", "bash"],
                ErrorKind::UnknownArgument,
            ),
        ] {
            let error = CompletionCli::try_parse_from(args)
                .err()
                .context("expected argument error")?;
            assert_eq!(error.kind(), kind, "{error}");
        }
        Ok(())
    }

    #[test]
    fn help_lists_every_supported_shell() -> Result<()> {
        let error = CompletionCli::try_parse_from(["completion", "--help"])
            .err()
            .context("help should stop argument parsing")?;
        assert_eq!(error.kind(), ErrorKind::DisplayHelp);
        let help = error.to_string();
        for shell in Shell::value_variants() {
            assert!(
                help.contains(&shell.to_string()),
                "missing {shell} in help: {help}"
            );
        }
        Ok(())
    }
}
