//! CLI command for creating new component projects from git repositories

use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, bail};
use clap::Args;
use serde_json::json;
use tokio::process::Command;
use tracing::{info, instrument};

use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    config::{load_config_from_file, locate_project_config},
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

        if let Some(new_cmd) =
            load_template_new_command(&output_dir).context("couldn't load template config")?
            && ctx.request_confirmation(format!(
                "Execute template setup command '{new_cmd}'? This may modify the new project."
            ))?
        {
            run_new_command(&new_cmd, &output_dir).await?;
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

fn load_template_new_command(project_dir: &Path) -> anyhow::Result<Option<String>> {
    let config_path = locate_project_config(project_dir);
    if !config_path.exists() {
        return Ok(None);
    }

    Ok(load_config_from_file(&config_path)?
        .new
        .and_then(|nc| nc.command))
}

/// Execute a template's `new.command` shell command in `working_dir`.
/// Returns an error if the command exits non-zero.
async fn run_new_command(new_cmd: &str, working_dir: &Path) -> anyhow::Result<()> {
    let (cmd_bin, first_arg) = {
        #[cfg(not(windows))]
        {
            ("sh", "-c")
        }

        #[cfg(windows)]
        {
            ("cmd", "/c")
        }
    };

    info!(command = new_cmd, "executing new command");
    let mut cmd = Command::new(cmd_bin)
        .args([first_arg, new_cmd])
        .stderr(Stdio::inherit())
        .stdout(Stdio::inherit())
        .current_dir(working_dir)
        .spawn()
        .context("failed to execute new command")?;

    let exit_status = cmd
        .wait()
        .await
        .context("failed to wait for new command to complete")?;

    if !exit_status.success() {
        bail!("new command '{new_cmd}' failed");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // `exit N` is a builtin in both POSIX `sh` and Windows `cmd`, so these tests
    // exercise the same code path that production uses on each platform.
    #[tokio::test]
    async fn run_new_command_bails_on_non_zero_exit() {
        let tempdir = tempfile::tempdir().unwrap();
        let err = run_new_command("exit 1", tempdir.path())
            .await
            .expect_err("non-zero exit should propagate as Err");
        assert!(
            err.to_string().contains("exit 1"),
            "error should name the failing command, got: {err}"
        );
    }

    #[tokio::test]
    async fn run_new_command_succeeds_on_zero_exit() {
        let tempdir = tempfile::tempdir().unwrap();
        run_new_command("exit 0", tempdir.path())
            .await
            .expect("zero-exit command should succeed");
    }

    #[test]
    fn template_new_command_is_absent_without_project_config() {
        let tempdir = tempfile::tempdir().unwrap();

        let new_cmd = load_template_new_command(tempdir.path())
            .expect("missing project config should not error");

        assert_eq!(new_cmd, None);
    }

    #[test]
    fn template_new_command_comes_from_project_config() {
        let tempdir = tempfile::tempdir().unwrap();
        let config_dir = tempdir.path().join(".wash");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("config.yaml"),
            "new:\n  command: cargo test\n",
        )
        .unwrap();

        let new_cmd =
            load_template_new_command(tempdir.path()).expect("project config should parse");

        assert_eq!(new_cmd.as_deref(), Some("cargo test"));
    }
}
