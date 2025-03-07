//! Generate wasmCloud projects (WebAssembly components, native capability providers, or contract interfaces) from templates

use std::{
    fmt, fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{anyhow, bail, Context, Result};
use console::style;
use genconfig::{Config, CONFIG_FILE_NAME};
use handlebars::Handlebars;
use indicatif::MultiProgress;
use serde::Serialize;
use tempfile::TempDir;
use tokio::process::Command;

pub mod emoji;
mod favorites;
mod genconfig;
mod git;
pub mod interactive;
pub mod project_variables;
use project_variables::fill_project_variables;
mod template;

type TomlMap = std::collections::BTreeMap<String, toml::Value>;
type ParamMap = std::collections::BTreeMap<String, serde_json::Value>;
/// pattern for project name and identifier are the same:
/// start with letter, then letter/digit/underscore/dash
const PROJECT_NAME_REGEX: &str = r"^([a-zA-Z][a-zA-Z0-9_-]+)$";

/// Type of project to be generated
#[derive(Debug, Default, Clone, Copy)]
pub enum ProjectKind {
    #[default]
    Component,
    Provider,
}

impl fmt::Display for ProjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Component => "component",
                Self::Provider => "provider",
            }
        )
    }
}

#[derive(Debug, Default, Clone)]
/// Contains information for generating a wasmCloud project, including fields for generating locally from a path
/// or from a remote git repository.
pub struct Project {
    /// Project kind
    pub kind: ProjectKind,

    /// Project name
    pub project_name: Option<String>,

    /// Optional path to file containing placeholder values
    pub values: Option<PathBuf>,

    /// Silent - do not prompt user. Placeholder values in the templates
    /// will be resolved from a '--values' file and placeholder defaults.
    pub silent: bool,

    /// Favorites file - to use for project selection
    pub favorites: Option<PathBuf>,

    /// Template name - name of template to use
    pub template_name: Option<String>,

    /// Don't run 'git init' on the new folder
    pub no_git_init: bool,

    /// Optional path for template project (alternative to --git)
    pub path: Option<PathBuf>,

    /// GitHub repository url. Requires 'git' to be installed in PATH.
    pub git: Option<String>,

    /// Optional subfolder of the git repository
    pub subfolder: Option<String>,

    /// Optional github branch. Defaults to "main"
    pub branch: Option<String>,
}

/// From a [Project] specification, generate a project of kind [`ProjectKind`]
///
/// # Arguments
/// - project: a [Project] struct containing required information to generate a wasmCloud project
///
/// # Returns
/// A Result containing a [`PathBuf`] with the location of the generated project
pub async fn generate_project(project: Project) -> Result<PathBuf> {
    validate(&project)?;

    // if user did not specify path to template dir or path to git repo,
    // pick one of the favorites for this kind
    let project = if project.path.is_none() && project.git.is_none() {
        let fav = favorites::pick_favorite(
            project.favorites.as_ref(),
            &project.kind,
            project.silent,
            project.template_name.as_ref(),
        )?;
        Project {
            path: fav.path.as_ref().map(PathBuf::from),
            git: fav.git,
            branch: fav.branch,
            subfolder: fav.subfolder,
            ..project
        }
    } else {
        project
    };

    make_project(project).await
}

fn validate(project: &Project) -> Result<()> {
    if project.path.is_some()
        && (project.git.is_some() || project.subfolder.is_some() || project.branch.is_some())
    {
        bail!("error in 'new {}' options: You may use --path or --git ( --branch, --subfolder ) to specify a template source, but not both. If neither is specified, you will be prompted to select a project template.",
            project.kind
        );
    }

    if project.git.is_some() || !project.no_git_init {
        if let Err(err) = std::process::Command::new("git")
            .args(["version"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
        {
            if err.kind() == ErrorKind::NotFound {
                bail!(
                    "error in 'new {}' options: required 'git' to be installed in PATH",
                    project.kind
                );
            }
            bail!(
                "error in 'new {}' options: failed to check 'git' command, {}",
                project.kind,
                err
            );
        }
    }

    if let Some(name) = &project.project_name {
        crate::lib::generate::project_variables::validate_project_name(name)
            .context("failed to validate project name")?;
    }
    if let Some(path) = &project.path {
        if !path.is_dir() {
            bail!(
                "error in --path option: '{}' is not an existing directory",
                &path.display()
            );
        }
    }
    if let Some(path) = &project.values {
        if !path.is_file() {
            bail!(
                "error in --values option: '{}' is not an existing file",
                &path.display()
            );
        }
    }
    if let Some(path) = &project.favorites {
        if !path.is_file() {
            bail!(
                "error in --favorites option: '{}' is not an existing file",
                &path.display()
            );
        }
    }
    Ok(())
}

fn any_msg(s1: &str, s2: &str) -> anyhow::Error {
    anyhow!(
        "{} {} {}",
        emoji::ERROR,
        style(s1).bold().red(),
        style(s2).bold().red()
    )
}

fn any_warn(s: &str) -> anyhow::Error {
    anyhow!("{} {}", emoji::WARN, style(s).bold().red())
}

async fn make_project(project: Project) -> std::result::Result<PathBuf, anyhow::Error> {
    // load optional values file
    let mut values = if let Some(values_file) = &project.values {
        let string = fs::read_to_string(values_file)
            .with_context(|| format!("reading values file {}", &values_file.display()))?;
        let tm = toml::from_str::<TomlMap>(&string)
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
        resolve_project_name(&values.get("project-name"), &project.project_name.as_ref())?;
    values.insert(
        "project-name".into(),
        project_name.user_input.clone().into(),
    );
    values.insert(
        "project-type".into(),
        serde_json::Value::String(project.kind.to_string()),
    );
    let project_dir = resolve_project_dir(&project_name)?;

    // select the template from args or a favorite file,
    // and copy its contents into a local folder
    let (template_base_dir, template_folder) = prepare_local_template(&project).await?;

    // read configuration file `project-generate.toml` from template.
    let project_config_path = fs::canonicalize(
        locate_project_config_file(CONFIG_FILE_NAME, &template_base_dir, &project.subfolder)
            .with_context(|| {
                format!(
                    "Invalid template folder: Required configuration file `{CONFIG_FILE_NAME}` is missing."
                )
            })?,
    )?;
    let mut config = Config::from_path(&project_config_path)?;
    // prevent copying config file to project dir by adding it to the exclude list
    config.exclude(
        if project_config_path.starts_with(&template_folder) {
            project_config_path.strip_prefix(&template_folder)?
        } else {
            &project_config_path
        }
        .to_string_lossy()
        .to_string(),
    );

    // resolve all project values, prompting if necessary,
    // and expanding templates in default values
    let renderer = Handlebars::default();
    let undefined =
        fill_project_variables(&config, &mut values, &renderer, project.silent, |slot| {
            crate::lib::generate::interactive::variable(slot)
        })?;
    if !undefined.is_empty() {
        return Err(any_msg("The following variables were not defined. Either add them to the --values file, or disable --silent: {}",
            &undefined.join(",")
        ));
    }

    println!(
        "{} {}{}",
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

    if !project.no_git_init {
        let cmd_out = Command::new("git")
            .args(["init", "--initial-branch", "main", "."])
            .current_dir(tokio::fs::canonicalize(&project_dir).await?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?
            .wait_with_output()
            .await?;
        if !cmd_out.status.success() {
            bail!(
                "git init error: {}",
                String::from_utf8_lossy(&cmd_out.stderr)
            );
        }
    }

    pbar.clear().ok();

    println!(
        "{} {} {} {}",
        emoji::SPARKLE,
        style("Done!").bold().green(),
        style("New project created").bold(),
        style(&project_dir.display()).underlined()
    );

    Ok(project_dir)
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
    subfolder: &Option<String>,
) -> Result<PathBuf>
where
    T: AsRef<Path>,
{
    let template_folder = template_folder.as_ref().to_path_buf();
    let mut search_folder = subfolder
        .as_ref()
        .map_or_else(|| template_folder.clone(), |s| template_folder.join(s));
    loop {
        let file_path = search_folder.join(name);
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

async fn prepare_local_template(project: &Project) -> Result<(TempDir, PathBuf)> {
    let (template_base_dir, template_folder) = match (&project.git, &project.path) {
        (Some(url), None) => {
            let template_base_dir = tempfile::tempdir()
                .map_err(|e| any_msg("Creating temp folder for staging:", &e.to_string()))?;

            println!(
                "{} {} {}{}{}",
                emoji::WRENCH,
                style("Cloning template from repo").bold(),
                style(url).bold().yellow(),
                project.subfolder.clone().map_or_else(
                    || style(String::new()),
                    |s| style(format!(
                        " {} {}",
                        style("subfolder").bold(),
                        style(s).bold().yellow()
                    ))
                ),
                style("...").bold()
            );

            git::clone_git_template(git::CloneTemplate {
                clone_tmp: template_base_dir.path().to_path_buf(),
                repo_url: url.to_string(),
                sub_folder: project.subfolder.clone(),
                repo_branch: project.branch.clone().unwrap_or_else(|| "main".to_string()),
            })
            .await?;
            let template_folder = resolve_template_dir(&template_base_dir, project)?;
            (template_base_dir, template_folder)
        }
        (None, Some(_)) => {
            let template_base_dir = copy_path_template_into_temp(project)?;
            let template_folder = template_base_dir.path().into();
            (template_base_dir, template_folder)
        }
        _ => {
            bail!(
                "{} {} {} {}",
                style("Please specify either").bold(),
                style("--git <repo>").bold().yellow(),
                style("or").bold(),
                style("--path <path>").bold().yellow(),
            )
        }
    };
    Ok((template_base_dir, template_folder))
}

fn resolve_template_dir(template_base_dir: &TempDir, project: &Project) -> Result<PathBuf> {
    match &project.subfolder {
        Some(subfolder) => {
            let template_base_dir = fs::canonicalize(template_base_dir.path())
                .map_err(|e| any_msg("Invalid template path:", &e.to_string()))?;
            let mut template_dir = template_base_dir.clone();
            // NOTE(thomastaylor312): Yeah, this is weird, but if you just `join` the PathBuf here
            // then you end up with mixed slashes, which doesn't work when file paths are
            // canonicalized on Windows
            template_dir.extend(PathBuf::from(subfolder).iter());
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
                "{} {} {}{}",
                emoji::WRENCH,
                style("Using template subfolder").bold(),
                style(subfolder).bold().yellow(),
                style("...").bold()
            );
            Ok(template_dir)
        }
        None => Ok(template_base_dir.path().to_owned()),
    }
}

fn copy_path_template_into_temp(project: &Project) -> Result<TempDir> {
    let path_clone_dir = tempfile::tempdir()
        .map_err(|e| any_msg("Creating temp folder for staging:", &e.to_string()))?;
    // args.path is already Some() when we get here
    let path = project.path.as_ref().unwrap();
    if !path.is_dir() {
        return Err(any_msg(&format!("template path {} not found - please try another template or fix the favorites path", &path.display()),""));
    }
    copy_dir_all(path, path_clone_dir.path())
        .with_context(|| format!("copying template project from {}", &path.display()))?;
    Ok(path_clone_dir)
}

fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
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

fn resolve_project_dir(name: &ProjectName) -> Result<PathBuf> {
    let dir_name = name.kebab_case();

    let project_dir = std::env::current_dir()
        .unwrap_or_else(|_e| ".".into())
        .join(dir_name);

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
struct ProjectName {
    user_input: String,
}

impl ProjectName {
    fn new(name: impl Into<String>) -> Self {
        Self {
            user_input: name.into(),
        }
    }

    fn kebab_case(&self) -> String {
        use heck::ToKebabCase as _;
        self.user_input.to_kebab_case()
    }
}
