use std::{collections::HashMap, path::PathBuf};

use anyhow::Result;
use clap::{Args, Subcommand};
use serde_json::json;
use wash_lib::{
    cli::CommandOutput,
    generate::{generate_project, Project, ProjectKind},
};

/// Create a new project from template
#[derive(Debug, Clone, Subcommand)]
pub enum NewCliCommand {
    /// Generate actor project
    #[clap(name = "actor")]
    Actor(NewProjectArgs),

    /// Generate a new interface project
    #[clap(name = "interface")]
    Interface(NewProjectArgs),

    /// Generate a new capability provider project
    #[clap(name = "provider")]
    Provider(NewProjectArgs),
}

#[derive(Args, Debug, Default, Clone)]
pub struct NewProjectArgs {
    /// Project name
    #[clap(help = "Project name")]
    pub(crate) project_name: Option<String>,

    /// Github repository url. Requires 'git' to be installed in PATH.
    #[clap(long)]
    pub(crate) git: Option<String>,

    /// Optional subfolder of the git repository
    #[clap(long, alias = "subdir")]
    pub(crate) subfolder: Option<String>,

    /// Optional github branch. Defaults to "main"
    #[clap(long)]
    pub(crate) branch: Option<String>,

    /// Optional path for template project (alternative to --git)
    #[clap(short, long)]
    pub(crate) path: Option<PathBuf>,

    /// Optional path to file containing placeholder values
    #[clap(short, long)]
    pub(crate) values: Option<PathBuf>,

    /// Silent - do not prompt user. Placeholder values in the templates
    /// will be resolved from a '--values' file and placeholder defaults.
    #[clap(long)]
    pub(crate) silent: bool,

    /// Favorites file - to use for project selection
    #[clap(long)]
    pub(crate) favorites: Option<PathBuf>,

    /// Template name - name of template to use
    #[clap(short, long)]
    pub(crate) template_name: Option<String>,

    /// Don't run 'git init' on the new folder
    #[clap(long)]
    pub(crate) no_git_init: bool,
}

impl From<NewCliCommand> for Project {
    fn from(cmd: NewCliCommand) -> Project {
        let (args, kind) = match cmd {
            NewCliCommand::Actor(args) => (args, ProjectKind::Actor),
            NewCliCommand::Interface(args) => (args, ProjectKind::Interface),
            NewCliCommand::Provider(args) => (args, ProjectKind::Provider),
        };

        Project {
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

pub(crate) async fn handle_command(cmd: NewCliCommand) -> Result<CommandOutput> {
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
}
