//! smithy model lint and validation
//!
use crate::generate::emoji;
use anyhow::anyhow;
use atelier_core::model::Model;
use console::style;
use std::path::PathBuf;
use structopt::StructOpt;
use weld_codegen::{
    config::{CodegenConfig, ModelSource, OutputLanguage},
    default_config, sources_to_model,
};

type TomlValue = toml::Value;
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

/// Generate code from smithy IDL files
#[derive(Debug, StructOpt, Clone)]
#[structopt(name = "gen")]
pub(crate) struct GenerateCli {
    #[structopt(flatten)]
    opt: GenerateOptions,
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

/// Generate code from smithy IDL files
#[derive(Debug, Clone, StructOpt)]
pub(crate) struct GenerateOptions {
    /// Configuration file (toml). Defaults to "./codegen.toml"
    #[structopt(short, long)]
    config: Option<PathBuf>,

    /// Output directory, defaults to current directory
    #[structopt(short, long)]
    output_dir: Option<PathBuf>,

    /// Optionally, load templates from this folder.
    /// Each template file name is its template name, for example, "header.hbs"
    /// is registered with the name "header"
    #[structopt(short = "T", long)]
    template_dir: Option<PathBuf>,

    /// Output language(s) to generate. May be specified more than once
    /// If not specified, all languages in config file will be generated (`-l html -l rust`)
    // number_of_values forces the user to use '-l' for each item
    #[structopt(short, long, number_of_values = 1)]
    lang: Vec<OutputLanguage>,

    /// Additional defines in the form of key=value to be passed to renderer
    /// Use `-D key=value` for each term to be added.
    #[structopt(short = "D", parse(try_from_str = parse_key_val), number_of_values = 1)]
    defines: Vec<(String, TomlValue)>,

    /// Enable verbose logging
    #[structopt(short, long)]
    verbose: bool,

    /// model files to process. Must specify either on command line or in 'models' array in codegen.toml
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

pub(crate) fn handle_gen_command(
    command: GenerateCli,
) -> Result<String, Box<dyn ::std::error::Error>> {
    let opt = command.opt;
    if let Some(ref tdir) = opt.template_dir {
        if !tdir.is_dir() {
            return Err("template_dir parameter must be an existing directory".into());
        }
    }
    let output_dir = match &opt.output_dir {
        Some(pb) => pb.to_owned(),
        _ => PathBuf::from("."),
    };
    let verbose = match opt.verbose {
        true => 1u8,
        false => 0u8,
    };
    let mut config = select_config(&opt.config)?;
    if !opt.lang.is_empty() {
        config.output_languages = opt.lang.clone()
    }
    let mut input_models = Vec::new();
    std::mem::swap(&mut config.models, &mut input_models);
    let model = build_model(opt.input, input_models, config.base_dir.clone(), verbose)?;

    let templates = if let Some(ref tdir) = opt.template_dir {
        println!(
            "{} {} {}",
            emoji::INFO,
            style("Importing templates from ").bold(),
            style(&tdir.display()).underlined()
        );
        weld_codegen::templates_from_dir(tdir)?
    } else {
        Vec::new()
    };

    let g = weld_codegen::Generator::default();
    g.gen(Some(&model), config, templates, &output_dir, opt.defines)?;

    Ok(String::new())
}

/// Parse a single key-value pair into (String,TomlValue)
fn parse_key_val(
    s: &str,
) -> Result<(String, TomlValue), Box<dyn std::error::Error + Send + Sync + 'static>> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
    Ok((s[..pos].to_string(), as_toml(&s[pos + 1..])))
}

// quick and easy conversion to toml for bool, int, or string
fn as_toml(s: &str) -> TomlValue {
    if s == "true" {
        return true.into();
    }
    if s == "false" {
        return false.into();
    }
    if let Ok(num) = s.parse::<i32>() {
        return num.into();
    };
    TomlValue::String(s.to_string())
}
