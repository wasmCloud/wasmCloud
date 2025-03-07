use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde_json::json;
use crate::lib::{
    cli::CommandOutput,
    generate::{generate_project, Project, ProjectKind},
};

/// Create a new project from template
#[derive(Debug, Clone, Subcommand)]
pub enum NewCliCommand {
    /// Generate a wasmCloud component project
    #[clap(name = "component")]
    Component(NewProjectArgs),

    /// Generate a new capability provider project
    #[clap(name = "provider")]
    Provider(NewProjectArgs),
}

#[derive(Args, Debug, Default, Clone)]
pub struct NewProjectArgs {
    /// Project name
    #[clap(help = "Project name")]
    pub project_name: Option<String>,

    /// GitHub repository url. Requires 'git' to be installed in PATH.
    #[clap(long)]
    pub git: Option<String>,

    /// Optional subfolder of the git repository
    #[clap(long, alias = "subdir")]
    pub subfolder: Option<String>,

    /// Optional github branch. Defaults to "main"
    #[clap(long)]
    pub branch: Option<String>,

    /// Optional path for template project (alternative to --git)
    #[clap(short, long)]
    pub path: Option<PathBuf>,

    /// Optional path to file containing placeholder values
    #[clap(short, long)]
    pub values: Option<PathBuf>,

    /// Silent - do not prompt user. Placeholder values in the templates
    /// will be resolved from a '--values' file and placeholder defaults.
    #[clap(long)]
    pub silent: bool,

    /// Favorites file - to use for project selection
    #[clap(long)]
    pub favorites: Option<PathBuf>,

    /// Template name - name of template to use
    #[clap(short, long)]
    pub template_name: Option<String>,

    /// Don't run 'git init' on the new folder
    #[clap(long)]
    pub no_git_init: bool,
}

impl From<NewCliCommand> for Project {
    fn from(cmd: NewCliCommand) -> Self {
        let (args, kind) = match cmd {
            NewCliCommand::Component(args) => (args, ProjectKind::Component),
            NewCliCommand::Provider(args) => (args, ProjectKind::Provider),
        };

        Self {
            kind,
            project_name: args.project_name,
            values: args.values,
            silent: args.silent,
            favorites: args.favorites,
            template_name: args.template_name,
            no_git_init: args.no_git_init,
            path: args.path,
            git: args.git,
            subfolder: args.subfolder,
            branch: args.branch,
        }
    }
}

pub async fn handle_command(cmd: NewCliCommand) -> Result<CommandOutput> {
    generate_project(cmd.into())
        .await
        .map(|path| CommandOutput {
            map: HashMap::from([(
                "project_path".to_string(),
                json!(path.to_string_lossy().to_string()),
            )]),
            text: format!(
                "Project generated and is located at: {}",
                path.to_string_lossy()
            ),
        })
        .context("Failed to generate project")
}
