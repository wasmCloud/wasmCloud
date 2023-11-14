//! Generate shell completion files

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use clap_complete::{generator::generate_to, shells::Shell};
use wash_lib::cli::CommandOutput;
use wash_lib::config::cfg_dir;

const TOKEN_FILE: &str = ".completion_suggested";
const COMPLETION_DOC_URL: &str =
    "https://github.com/wasmCloud/wasmCloud/blob/main/crates/wash-cli/Completions.md";

fn instructions() -> String {
    format!(
        "For instructions on setting up auto-complete for your shell, please see '{}'",
        COMPLETION_DOC_URL
    )
}

#[derive(Debug, Clone, Args)]
pub struct CompletionOpts {
    /// Output directory (default '.')
    #[clap(short = 'd', long = "dir")]
    dir: Option<PathBuf>,

    /// Shell
    #[clap(name = "shell", subcommand)]
    shell: ShellSelection,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ShellSelection {
    /// generate completions for Zsh
    Zsh,
    /// generate completions for Bash
    Bash,
    /// generate completions for Fish
    Fish,
    /// generate completions for PowerShell
    PowerShell,
}

/// Displays a message one time after wash install
pub fn first_run_suggestion() -> Result<Option<String>> {
    let cfg_dir = cfg_dir()?;
    let token = cfg_dir.join(TOKEN_FILE);
    if token.is_file() {
        return Ok(None);
    }
    let _ = std::fs::File::create(token).with_context(|| {
        format!(
            "can't create completion first-run token in {}",
            cfg_dir.display()
        )
    })?;
    Ok(Some(format!(
        "Congratulations on installing wash!  Shell auto-complete is available. {}",
        instructions()
    )))
}

pub fn handle_command(
    opts: CompletionOpts,
    mut command: clap::builder::Command,
) -> Result<CommandOutput> {
    let output_dir = opts.dir.unwrap_or_else(|| PathBuf::from("."));

    let shell = match opts.shell {
        ShellSelection::Zsh => Shell::Zsh,
        ShellSelection::Bash => Shell::Bash,
        ShellSelection::Fish => Shell::Fish,
        ShellSelection::PowerShell => Shell::PowerShell,
    };

    match generate_to(shell, &mut command, "wash", &output_dir) {
        Ok(path) => {
            let mut map = HashMap::new();
            map.insert(
                "path".to_string(),
                path.to_string_lossy().to_string().into(),
            );
            Ok(CommandOutput::new(
                format!(
                    "Generated completion file: {}. {}",
                    path.display(),
                    instructions()
                ),
                map,
            ))
        }
        Err(e) => bail!(
            "generating shell completion file in folder '{}': {}",
            output_dir.display(),
            e
        ),
    }
}
