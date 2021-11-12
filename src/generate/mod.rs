//! ## Project generation from templates
//!
//! This module contains code for `wash new ...` commands
//! to creating a new project from a template.
//!
//! This module has some functionality (and code) in common with
//! `cargo-generate`, which can also can create a new project from
//! a template folder on disk or from a template in github.
//! We are thankful for the cargo-generate project and its contributors,
//! and acknowledge that the following functionality is
//! largely copied from that project:
//! - github downloads, config file loading, directory tree traversal,
//!   and terminal io (progress bars, emoji, and variable prompts)
//!
//! Some of the differences between this and cargo-generate:
//! - Because it is integrated with wash, which has binary distributions,
//!   users do not need to have cargo or the rust toolchain installed.
//! - This implementation is intended to support more target languages,
//!   and tries to be less rust/cargo centric.
//! - uses handlebars templates instead of liquid, for consistency
//!   with templates used for code generation from smithy files.
//!   The syntax between these engines is very similar.
//!   Handlebars (currently) has greater usage in the rust community,
//!   and is more familiar with developers of javascript and other languages.
//! - categorization of templates by kind: actor, interface, and provider.
//! - project config file includes optional table for renaming files
//! - template expansion may occur within file contents,
//!   within file names, and within default values.
//!   (cargo-generate supports the first 2/3)
//! - fewer cli options for a simpler user experience
//! - does not perform git init on the generated project
//!
// Some of this code is based on code from cargo-generate
//   source: https://github.com/cargo-generate/cargo-generate
//   version: 0.9.0
//   license: MIT/Apache-2.0
//

use anyhow::{anyhow, Context, Result};
use config::{Config, CONFIG_FILE_NAME};
use console::style;
use git::GitConfig;
use heck::KebabCase;
use indicatif::MultiProgress;
use project_variables::*;
use serde::Serialize;
use std::borrow::Borrow;
use std::{
    fmt, fs,
    path::{Path, PathBuf},
};
use structopt::StructOpt;
use tempfile::TempDir;
use weld_codegen::render::Renderer;

mod config;
pub(crate) mod emoji;
mod favorites;
mod git;
pub(crate) mod interactive;
pub(crate) mod project_variables;
mod template;

pub(crate) type TomlMap = std::collections::BTreeMap<String, toml::Value>;
pub(crate) type ParamMap = std::collections::BTreeMap<String, serde_json::Value>;
/// pattern for project name and identifier are the same:
/// start with letter, then letter/digit/underscore/dash
pub(crate) const PROJECT_NAME_REGEX: &str = r"^([a-zA-Z][a-zA-Z0-9_-]+)$";

/// Create a new project from template
#[derive(Debug, Clone, StructOpt)]
pub(crate) struct NewCli {
    #[structopt(flatten)]
    command: NewCliCommand,
}

impl NewCli {
    pub(crate) fn command(self) -> NewCliCommand {
        self.command
    }
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) enum NewCliCommand {
    /// Generate actor project
    #[structopt(name = "actor")]
    Actor(NewProjectArgs),

    /// Generate a new interface project
    #[structopt(name = "interface")]
    Interface(NewProjectArgs),

    /// Generate a new capability provider project
    #[structopt(name = "provider")]
    Provider(NewProjectArgs),
}

/// Type of project to be generated
#[derive(Debug)]
pub(crate) enum ProjectKind {
    Actor,
    Interface,
    Provider,
}

impl From<&NewCliCommand> for ProjectKind {
    fn from(cmd: &NewCliCommand) -> ProjectKind {
        match cmd {
            NewCliCommand::Actor(_) => ProjectKind::Actor,
            NewCliCommand::Interface(_) => ProjectKind::Interface,
            NewCliCommand::Provider(_) => ProjectKind::Provider,
        }
    }
}

impl fmt::Display for ProjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                ProjectKind::Actor => "actor",
                ProjectKind::Interface => "interface",
                ProjectKind::Provider => "provider",
            }
        )
    }
}

#[derive(StructOpt, Debug, Default, Clone)]
pub(crate) struct NewProjectArgs {
    /// Project name
    #[structopt(help = "Project name")]
    pub(crate) project_name: Option<String>,

    /// Github repository url
    #[structopt(long)]
    pub(crate) git: Option<String>,

    /// Optional subfolder of the git repository
    #[structopt(long, alias = "subdir")]
    pub(crate) subfolder: Option<PathBuf>,

    /// Optional github branch
    #[structopt(long)]
    pub(crate) branch: Option<String>,

    /// Optional path for template project
    #[structopt(short, long)]
    pub(crate) path: Option<PathBuf>,

    /// optional path to file containing placeholder values
    #[structopt(short, long)]
    pub(crate) values: Option<PathBuf>,

    /// ssh identity file, for ssh authentication
    #[structopt(short = "i", long)]
    pub(crate) ssh_identity: Option<PathBuf>,

    /// silent - do not prompt user. Placeholder values in the templates
    /// will be resolved from a '--values' file and placeholder defaults.
    #[structopt(long)]
    pub(crate) silent: bool,

    /// favorites file - to use for project selection
    #[structopt(long)]
    pub(crate) favorites: Option<PathBuf>,

    /// template name - name of template to use
    #[structopt(short, long)]
    pub(crate) template_name: Option<String>,
}

pub(crate) fn handle_command(
    command: NewCliCommand,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    validate(&command)?;

    let kind = ProjectKind::from(&command);
    let cmd = match command {
        NewCliCommand::Actor(gc) | NewCliCommand::Interface(gc) | NewCliCommand::Provider(gc) => gc,
    };

    // if user did not specify path to template dir or path to git repo,
    // pick one of the favorites for this kind
    let cmd = if cmd.path.is_none() && cmd.git.is_none() {
        let fav = favorites::pick_favorite(
            cmd.favorites.as_ref(),
            &kind,
            cmd.silent,
            cmd.template_name.as_ref(),
        )?;
        NewProjectArgs {
            path: fav.path.as_ref().map(PathBuf::from),
            git: fav.git.clone(),
            branch: fav.branch.clone(),
            subfolder: fav.subfolder.as_ref().map(PathBuf::from),
            ..cmd
        }
    } else {
        cmd
    };

    make_project(kind, cmd)?;
    Ok(String::new())
}

fn validate(command: &NewCliCommand) -> Result<()> {
    let cmd = match command {
        NewCliCommand::Actor(gc) | NewCliCommand::Interface(gc) | NewCliCommand::Provider(gc) => gc,
    };

    if cmd.path.is_some() && (cmd.git.is_some() || cmd.subfolder.is_some() || cmd.branch.is_some())
    {
        return Err(anyhow!("Error in 'new {}' options: You may use --path or --git ( --branch, --subfolder ) to specify a template source, but not both. If neither is specified, you will be prompted to select a project template.",
            &ProjectKind::from(command)
        ));
    }
    if let Some(name) = &cmd.project_name {
        crate::generate::project_variables::validate_project_name(name)?;
    }
    if let Some(path) = &cmd.path {
        if !path.is_dir() {
            return Err(anyhow!(
                "Error in --path option: '{}' is not an existing directory",
                &path.display()
            ));
        }
    }
    if let Some(path) = &cmd.values {
        if !path.is_file() {
            return Err(anyhow!(
                "Error in --values option: '{}' is not an existing file",
                &path.display()
            ));
        }
    }
    if let Some(path) = &cmd.ssh_identity {
        if !path.is_file() {
            return Err(anyhow!(
                "Error in --ssh_identity option: '{}' is not an existing file",
                &path.display()
            ));
        }
    }
    if let Some(path) = &cmd.favorites {
        if !path.is_file() {
            return Err(anyhow!(
                "Error in --favorites option: '{}' is not an existing file",
                &path.display()
            ));
        }
    }
    Ok(())
}

pub(crate) fn any_error(s: &str, e: anyhow::Error) -> anyhow::Error {
    anyhow!(
        "{} {} {}",
        emoji::ERROR,
        style(s).bold().red(),
        style(e).bold().red()
    )
}

pub(crate) fn any_msg(s1: &str, s2: &str) -> anyhow::Error {
    anyhow!(
        "{} {} {}",
        emoji::ERROR,
        style(s1).bold().red(),
        style(s2).bold().red()
    )
}

pub(crate) fn any_warn(s: &str) -> anyhow::Error {
    anyhow!("{} {}", emoji::WARN, style(s).bold().red())
}

pub(crate) fn make_project(
    kind: ProjectKind,
    args: NewProjectArgs,
) -> std::result::Result<(), anyhow::Error> {
    let _ = env_logger::try_init();

    // load optional values file
    let mut values = if let Some(values_file) = &args.values {
        let bytes = fs::read(&values_file)
            .with_context(|| format!("reading values file {}", &values_file.display()))?;
        let tm = toml::from_slice::<TomlMap>(&bytes)
            .with_context(|| format!("parsing values file {}", &values_file.display()))?;
        if let Some(toml::Value::Table(values)) = tm.get("values") {
            toml_to_json(values)?
        } else {
            ParamMap::default()
        }
    } else {
        ParamMap::default()
    };

    let project_name =
        resolve_project_name(&values.get("project-name"), &args.project_name.as_ref())?;
    values.insert(
        "project-name".into(),
        project_name.user_input.clone().into(),
    );
    values.insert(
        "project-type".into(),
        serde_json::Value::String(kind.to_string()),
    );
    let project_dir = resolve_project_dir(&project_name)?;

    // select the template from args or a favorite file,
    // and copy its contents into a local folder
    let (template_base_dir, template_folder, _branch) = prepare_local_template(&args)?;

    // read configuration file `project-generate.toml` from template.
    let project_config_path = fs::canonicalize(
        locate_project_config_file(CONFIG_FILE_NAME, &template_base_dir, &args.subfolder)
            .with_context(|| {
                format!(
                    "Invalid template folder: Required configuration file `{}` is missing.",
                    CONFIG_FILE_NAME
                )
            })?,
    )?;
    let mut config = Config::from_path(&project_config_path)?;
    // prevent copying config file to project dir by adding it to the exclude list
    config.exclude(
        project_config_path
            .strip_prefix(&template_folder)?
            .to_string_lossy()
            .to_string(),
    );

    // resolve all project values, prompting if necessary,
    // and expanding templates in default values
    let renderer = Renderer::default();
    let undefined = fill_project_variables(&config, &mut values, &renderer, args.silent, |slot| {
        crate::generate::interactive::variable(slot)
    })?;
    if !undefined.is_empty() {
        return Err(any_msg("The following variables were not defined. Either add them to the --values file, or disable --silent: {}",
            &undefined.join(",")
        ));
    }

    println!(
        "{} {} {}",
        emoji::WRENCH,
        style("Generating template").bold(),
        style("...").bold()
    );

    let template_config = config.template.unwrap_or_default();
    let mut pbar = MultiProgress::new();
    template::process_template_dir(
        &template_folder,
        &project_dir,
        &template_config,
        &renderer,
        &values,
        &mut pbar,
    )
    .map_err(|e| any_msg("generating project from templates:", &e.to_string()))?;
    pbar.join().unwrap();

    println!(
        "{} {} {} {}",
        emoji::SPARKLE,
        style("Done!").bold().green(),
        style("New project created").bold(),
        style(&project_dir.display()).underlined()
    );
    Ok(())
}

// convert from TOML map to JSON map
fn toml_to_json<T: Serialize>(map: &T) -> Result<ParamMap> {
    let s = serde_json::to_string(map)?;
    let value: ParamMap = serde_json::from_str(&s)?;
    Ok(value)
}

/// Finds template configuration in subfolder or a parent.
/// Returns error if no configuration was found
fn locate_project_config_file<T>(
    name: &str,
    template_folder: T,
    subfolder: &Option<PathBuf>,
) -> Result<PathBuf>
where
    T: AsRef<Path>,
{
    let template_folder = template_folder.as_ref().to_path_buf();
    let mut search_folder = subfolder
        .as_ref()
        .map_or_else(|| template_folder.to_owned(), |s| template_folder.join(s));
    loop {
        let file_path = search_folder.join(name.borrow());
        if file_path.exists() {
            return Ok(file_path);
        }
        if search_folder == template_folder {
            return Err(any_msg("File not found within template", ""));
        }
        search_folder = search_folder
            .parent()
            .ok_or_else(|| {
                any_msg(
                    "Missing Config:",
                    &format!(
                        "did not find {} in {} or any of its parents.",
                        &search_folder.display(),
                        CONFIG_FILE_NAME
                    ),
                )
            })?
            .to_path_buf();
    }
}

pub(crate) fn prepare_local_template(args: &NewProjectArgs) -> Result<(TempDir, PathBuf, String)> {
    let (template_base_dir, template_folder, branch) = match (&args.git, &args.path) {
        (Some(_), None) => {
            let (template_base_dir, branch) = clone_git_template_into_temp(args)?;
            let template_folder = resolve_template_dir(&template_base_dir, args)?;
            (template_base_dir, template_folder, branch)
        }
        (None, Some(_)) => {
            let template_base_dir = copy_path_template_into_temp(args)?;
            let branch = args.branch.clone().unwrap_or_else(|| String::from("main"));
            let template_folder = template_base_dir.path().into();
            (template_base_dir, template_folder, branch)
        }
        _ => {
            return Err(anyhow!(
                "{} {} {} {}",
                style("Please specify either").bold(),
                style("--git <repo>").bold().yellow(),
                style("or").bold(),
                style("--path <path>").bold().yellow(),
            ))
        }
    };
    Ok((template_base_dir, template_folder, branch))
}

fn resolve_template_dir(template_base_dir: &TempDir, args: &NewProjectArgs) -> Result<PathBuf> {
    match &args.subfolder {
        Some(subfolder) => {
            let template_base_dir = fs::canonicalize(template_base_dir.path())
                .map_err(|e| any_msg("Invalid template path:", &e.to_string()))?;
            let mut template_dir = template_base_dir.clone();
            // NOTE(thomastaylor312): Yeah, this is weird, but if you just `join` the PathBuf here
            // then you end up with mixed slashes, which doesn't work when file paths are
            // canonicalized on Windows
            template_dir.extend(subfolder.iter());
            let template_dir = fs::canonicalize(template_dir)
                .map_err(|e| any_msg("Invalid subfolder path:", &e.to_string()))?;

            if !template_dir.starts_with(&template_base_dir) {
                return Err(any_msg(
                    "Subfolder Error:",
                    "Invalid subfolder. Must be part of the template folder structure.",
                ));
            }
            if !template_dir.is_dir() {
                return Err(any_msg(
                    "Subfolder Error:",
                    "The specified subfolder must be a valid folder.",
                ));
            }

            println!(
                "{} {} `{}`{}",
                emoji::WRENCH,
                style("Using template subfolder").bold(),
                style(subfolder.display()).bold().yellow(),
                style("...").bold()
            );
            Ok(template_dir)
        }
        None => Ok(template_base_dir.path().to_owned()),
    }
}

fn copy_path_template_into_temp(args: &NewProjectArgs) -> Result<TempDir> {
    let path_clone_dir = tempfile::tempdir()
        .map_err(|e| any_msg("Creating temp folder for staging:", &e.to_string()))?;
    // args.path is already Some() when we get here
    let path = args.path.as_ref().unwrap();
    if !path.is_dir() {
        return Err(any_msg(&format!("template path {} not found - please try another template or fix the favorites path", &path.display()),""));
    }
    copy_dir_all(&path, &path_clone_dir.path())
        .with_context(|| format!("copying template project from {}", &path.display()))?;
    Ok(path_clone_dir)
}

fn clone_git_template_into_temp(args: &NewProjectArgs) -> Result<(TempDir, String)> {
    let git_clone_dir = tempfile::tempdir()
        .map_err(|e| any_msg("Creating temp folder for staging:", &e.to_string()))?;

    let remote = args
        .git
        .clone()
        .with_context(|| "Missing option git, path or a favorite")?;

    let git_config = GitConfig::new_abbr(
        remote.into(),
        args.branch.to_owned(),
        args.ssh_identity.clone(),
    )?;

    let branch =
        git::create(git_clone_dir.path(), git_config).map_err(|e| any_error("Git Error:", e))?;

    Ok((git_clone_dir, branch))
}

pub(crate) fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
    fn check_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
        if !dst.as_ref().exists() {
            return Ok(());
        }

        for src_entry in fs::read_dir(src)? {
            let src_entry = src_entry?;
            let dst_path = dst.as_ref().join(src_entry.file_name());
            let entry_type = src_entry.file_type()?;

            if entry_type.is_dir() {
                check_dir_all(src_entry.path(), dst_path)?;
            } else if entry_type.is_file() {
                if dst_path.exists() {
                    return Err(any_msg(
                        "File already exists:",
                        &dst_path.display().to_string(),
                    ));
                }
            } else {
                return Err(any_warn("Symbolic links not supported"));
            }
        }
        Ok(())
    }
    fn copy_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
        fs::create_dir_all(&dst)?;
        for src_entry in fs::read_dir(src)? {
            let src_entry = src_entry?;
            let dst_path = dst.as_ref().join(src_entry.file_name());
            let entry_type = src_entry.file_type()?;
            if entry_type.is_dir() {
                copy_dir_all(src_entry.path(), dst_path)?;
            } else if entry_type.is_file() {
                fs::copy(src_entry.path(), dst_path)?;
            }
        }
        Ok(())
    }

    check_dir_all(&src, &dst)?;
    copy_all(src, dst)
}

pub(crate) fn resolve_project_dir(name: &ProjectName) -> Result<PathBuf> {
    let dir_name = name.kebab_case();

    let project_dir = std::env::current_dir()
        .unwrap_or_else(|_e| ".".into())
        .join(&dir_name);

    if project_dir.exists() {
        Err(any_msg("Target directory already exists.", "aborting!"))
    } else {
        Ok(project_dir)
    }
}

fn resolve_project_name(
    value: &Option<&serde_json::Value>,
    arg: &Option<&String>,
) -> Result<ProjectName> {
    match (value, arg) {
        (_, Some(arg_name)) => Ok(ProjectName::new(arg_name.as_str())),
        (Some(serde_json::Value::String(val_name)), _) => Ok(ProjectName::new(val_name)),
        _ => Ok(ProjectName::new(interactive::name()?)),
    }
}

/// Stores user inputted name and provides convenience methods
/// for handling casing.
pub(crate) struct ProjectName {
    pub(crate) user_input: String,
}

impl ProjectName {
    pub(crate) fn new(name: impl Into<String>) -> ProjectName {
        ProjectName {
            user_input: name.into(),
        }
    }

    pub(crate) fn kebab_case(&self) -> String {
        self.user_input.to_kebab_case()
    }
}
