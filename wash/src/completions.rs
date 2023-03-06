//! Generate shell completion files
//!
use crate::{cfg::cfg_dir, CommandOutput};
use anyhow::anyhow;
use anyhow::Result;
use clap::{Args, Subcommand};
use clap_complete::{generator::generate_to, shells::Shell};
use std::collections::HashMap;
use std::path::PathBuf;

const TOKEN_FILE: &str = ".completion_suggested";
const COMPLETION_DOC_URL: &str = "https://github.com/wasmCloud/wash/blob/main/Completions.md";

fn instructions() -> String {
    format!(
        "For instructions on setting up auto-complete for your shell, please see '{}'",
        COMPLETION_DOC_URL
    )
}

#[derive(Debug, Clone, Args)]
pub(crate) struct CompletionOpts {
    /// Output directory (default '.')
    #[clap(short = 'd', long = "dir")]
    dir: Option<PathBuf>,

    /// Shell
    #[clap(name = "shell", subcommand)]
    shell: ShellSelection,
}

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum ShellSelection {
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
pub(crate) fn first_run_suggestion() -> Result<Option<String>> {
    let token = cfg_dir()?.join(TOKEN_FILE);
    if token.is_file() {
        return Ok(None);
    }
    let _ = std::fs::File::create(token).map_err(|e| {
        anyhow!(
            "can't create completion first-run token in {}: {}",
            // unwrap() ok because cfg_dir worked above
            cfg_dir().unwrap().display(),
            e
        )
    })?;
    Ok(Some(format!(
        "Congratulations on installing wash!  Shell auto-complete is available. {}",
        instructions()
    )))
}

pub(crate) fn handle_command(
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
        Err(e) => Err(anyhow!(
            "generating shell completion file in folder '{}': {}",
            output_dir.display(),
            e
        )),
    }
}
