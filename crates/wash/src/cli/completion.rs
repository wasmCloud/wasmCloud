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
