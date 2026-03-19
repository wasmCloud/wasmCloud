//! CLI command for creating new component projects from git repositories

use std::process::Stdio;

use anyhow::{Context, bail};
use clap::Args;
use serde_json::json;
use tokio::process::Command;
use tracing::{info, instrument};

use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    config::{Config, load_config},
    new::{clone_template, copy_dir_recursive, extract_subfolder},
};

/// Create a new component project from a git repository
#[derive(Args, Debug, Clone)]
pub struct NewCommand {
    /// Git repository URL to use as project template
    git: String,

    /// Project name and local directory to create (defaults to repository/subfolder name)
    #[arg(long)]
    name: Option<String>,

    /// Subdirectory within the git repository to use
    #[arg(long)]
    subfolder: Option<String>,

    /// Git reference (branch, tag, or commit) to checkout
    #[arg(long)]
    git_ref: Option<String>,
}

impl CliCommand for NewCommand {
    #[instrument(level = "debug", skip(self, ctx), name = "new")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let project_name = self.get_project_name();
        // Explicitly use project_dir from context instead of relying on working directory
        let output_dir = ctx.project_dir().join(&project_name);

        if output_dir.exists() {
            bail!("Output directory already exists: {}", output_dir.display());
        }

        info!(
            "Creating new project '{}' from git repository: {}",
            project_name, self.git
        );

        let tempdir = tempfile::tempdir().context("failed to create temp dir")?;

        // Clone the repository
        clone_template(&self.git, tempdir.path(), self.git_ref.as_deref())
            .await
            .context("failed to clone git repository")?;

        if let Some(subfolder) = &self.subfolder {
            // Extract subfolder if specified
            extract_subfolder(tempdir.path(), &output_dir, subfolder)
                .await
                .context("failed to extract subfolder")?;
        } else {
            // Copy instead of move as we might be on a different filesystem
            copy_dir_recursive(tempdir.path(), &output_dir)
                .await
                .context("failed to copy cloned repository to output directory")?;
        }

        // Check if the output directory has a wash config
        let template_config =
            load_config(&ctx.user_config_path(), Some(&output_dir), None::<Config>)
                .context("couldn't load template config")?;

        if let Some(new_cmd) = template_config.new.and_then(|nc| nc.command)
            && ctx.request_confirmation(format!(
                "Execute template setup command '{}'? This may modify the new project.",
                new_cmd
            ))?
        {
            let (cmd_bin, first_arg) = {
                #[cfg(not(windows))]
                {
                    ("sh".to_string(), "-c".to_string())
                }

                #[cfg(windows)]
                {
                    ("cmd".to_string(), "/c".to_string())
                }
            };

            let cmd_args = vec![first_arg, new_cmd.clone()];

            info!(command = new_cmd, "executing new command");
            let mut cmd = Command::new(cmd_bin)
                .args(cmd_args)
                .stderr(Stdio::inherit())
                .stdout(Stdio::inherit())
                .current_dir(&output_dir)
                .spawn()
                .context("failed to execute new command")?;

            let exit_status = cmd
                .wait()
                .await
                .context("failed to wait for new command to complete")?;

            if !exit_status.success() {
                bail!("new command '{}' failed", new_cmd);
            }
        }

        Ok(CommandOutput::ok(
            format!(
                "Project '{project_name}' created successfully at {}",
                output_dir.display()
            ),
            Some(json!({
                "name": project_name,
                "repository": self.git,
                "subfolder": self.subfolder,
                "output_dir": output_dir,
            })),
        ))
    }
}

impl NewCommand {
    /// Get project name from CLI args or derive from repository/subfolder
    fn get_project_name(&self) -> String {
        if let Some(ref name) = self.name {
            return name.clone();
        }

        // Try to derive name from subfolder first, then from repository URL
        if let Some(subfolder) = &self.subfolder {
            subfolder
                .split('/')
                .next_back()
                .unwrap_or("new-project")
                .to_string()
        } else {
            self.git
                .split('/')
                .next_back()
                .unwrap_or("new-project")
                .trim_end_matches(".git")
                .to_string()
        }
    }
}
