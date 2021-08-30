//! smithy model lint and validation
//!
use anyhow::anyhow;
use atelier_core::model::Model;
use std::path::PathBuf;
use structopt::StructOpt;
use weld_codegen::{
    config::{CodegenConfig, ModelSource},
    default_config, sources_to_model,
};

const CODEGEN_CONFIG_FILE: &str = "codegen.toml";

/// Perform lint checks on smithy models
#[derive(Debug, StructOpt, Clone)]
#[structopt(name = "lint")]
pub(crate) struct LintCli {
    #[structopt(flatten)]
    opt: LintOptions,
}

/// Perform validation checks on smithy models
#[derive(Debug, StructOpt, Clone)]
#[structopt(name = "validate")]
pub(crate) struct ValidateCli {
    #[structopt(flatten)]
    opt: ValidateOptions,
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct LintOptions {
    /// Configuration file. Defaults to "./codegen.toml".
    /// Used to get model files only if input files are not specified on the command line.
    #[structopt(short, long)]
    config: Option<PathBuf>,

    /// Enable verbose logging
    #[structopt(short, long)]
    verbose: bool,

    /// Input files to process (overrides codegen.toml)
    #[structopt(name = "input")]
    input: Vec<String>,
}

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct ValidateOptions {
    /// Configuration file. Defaults to "./codegen.toml".
    /// Used to get model files only if input files are not specified on the command line.
    #[structopt(short, long)]
    config: Option<PathBuf>,

    /// Enable verbose logging
    #[structopt(short, long)]
    verbose: bool,

    /// Input files to process (overrides codegen.toml)
    #[structopt(name = "input")]
    input: Vec<String>,
}

pub(crate) async fn handle_lint_command(
    command: LintCli,
) -> Result<String, Box<dyn ::std::error::Error>> {
    let opt = command.opt;
    let verbose = match opt.verbose {
        true => 1u8,
        false => 0u8,
    };
    use atelier_core::action::lint::{run_linter_actions, NamingConventions, UnwelcomeTerms};
    let config = select_config(&opt.config)?;
    let model = build_model(opt.input, config.models, config.base_dir, verbose)?;
    let report = run_linter_actions(
        &mut [
            Box::new(NamingConventions::default()),
            Box::new(UnwelcomeTerms::default()),
        ],
        &model,
        false,
    )
    .map_err(|e| anyhow!("lint error: {}", e.to_string()))?;

    cargo_atelier::report::report_action_issues(report, true)
        .map_err(|e| anyhow!("report error: {}", e))?;

    Ok(String::new())
}

pub(crate) async fn handle_validate_command(
    command: ValidateCli,
) -> Result<String, Box<dyn ::std::error::Error>> {
    use atelier_core::action::validate::{
        run_validation_actions, CorrectTypeReferences, NoUnresolvedReferences,
    };
    let opt = command.opt;
    let verbose = match opt.verbose {
        true => 1u8,
        false => 0u8,
    };
    let config = select_config(&opt.config)?;
    let model = build_model(opt.input, config.models, config.base_dir, verbose)?;

    /*
    // Unions are not supported because msgpack doesn't know how to serialize them
    expect_empty!(ix.unions, "Unions are not supported");
    // might support these in the future, but not yet
    expect_empty!(ix.resources, "Resources are not supported");
    // indicates a model error - probably typo or forgot to include a definition file
    expect_empty!(ix.unresolved, "types could not be determined");
     */

    let report = run_validation_actions(
        &mut [
            Box::new(CorrectTypeReferences::default()),
            Box::new(NoUnresolvedReferences::default()),
        ],
        &model,
        false,
    )
    .map_err(|e| anyhow!("validation error: {}", e.to_string()))?;
    cargo_atelier::report::report_action_issues(report, true)
        .map_err(|e| anyhow!("report error: {}", e))?;

    Ok(String::new())
}

/// build model from input files and/or files listed in codegen.toml.
/// Dependent models may be downloaded by a background thread.
fn build_model(
    input: Vec<String>,
    models: Vec<ModelSource>,
    base_dir: PathBuf,
    verbose: u8,
) -> Result<Model, anyhow::Error> {
    // The downloader crate (used by sources_to_model) creates a tokio Runtime
    // and calls block_on(), but since we're already in a Runtime created by main().
    // that panics. Using thread::spawn here allows the second Runtime.
    std::thread::spawn(move || {
        if input.is_empty() {
            sources_to_model(&models, &base_dir, verbose).map_err(|e| e.to_string())
        } else {
            inputs_to_model(&input, verbose).map_err(|e| e.to_string())
        }
    })
    .join()
    .map_err(|_| anyhow!("downloader thread paniced"))?
    .map_err(|e| anyhow!("{}", e))
}

/// build model from inputs files provided on the command line
fn inputs_to_model(inputs: &[String], verbose: u8) -> Result<Model, anyhow::Error> {
    use std::str::FromStr;
    let inputs = inputs
        .iter()
        // unwrap below ok because this from_str is Infallible
        .map(|s| ModelSource::from_str(s).unwrap())
        .collect::<Vec<ModelSource>>();
    let current_dir = PathBuf::from(".");
    Ok(sources_to_model(&inputs, &current_dir, verbose)?)
}

/// identify config file from command-line, current-directory, or built-in default
/// Returns the configuration, and whether default was used.
fn select_config(opt_config: &Option<PathBuf>) -> Result<CodegenConfig, anyhow::Error> {
    // if --config is not specified in the command-line, try the current directory.
    // if it's not found use the default
    let (cfile, folder) = if let Some(path) = &opt_config {
        (
            std::fs::read_to_string(path)
                .map_err(|e| anyhow!("reading config file {}: {}", path.display(), e))?,
            path.parent().unwrap().to_path_buf(),
        )
    } else if PathBuf::from(CODEGEN_CONFIG_FILE).is_file() {
        (
            std::fs::read_to_string(CODEGEN_CONFIG_FILE)
                .map_err(|e| anyhow!("reading config file {}.toml: {}", CODEGEN_CONFIG_FILE, e))?,
            PathBuf::from("."),
        )
    } else {
        (default_config().to_string(), PathBuf::from("."))
    };
    let folder = std::fs::canonicalize(folder)?;
    let mut config = cfile.parse::<CodegenConfig>()?;
    config.base_dir = folder;

    Ok(config)
}
