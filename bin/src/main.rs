use anyhow::{anyhow, Result};
use atelier_core::model::Model;
use clap::{self, Clap};
use std::{path::PathBuf, str::FromStr, string::ToString};
use toml::Value as TomlValue;
use weld_codegen::{
    config::{CodegenConfig, ModelSource, OutputLanguage},
    sources_to_model,
    //load_model,
    //model_sources,
    templates_from_dir,
    //render::{RenderConfig, RenderError, Renderer},
    Generator,
};

const DEFAULT_CONFIG: &str = include_str!("../../codegen/templates/codegen.toml");

// default project name, can be overridden in gen create with `-D project_name="foo"`
const SAMPLE_PROJECT_NAME: &str = "my_project";

// default project version, can be overridden in gen create with `-D project_version="1.1.0"`
const SAMPLE_PROJECT_VERSION: &str = "0.1.0";

// for creating new interface project:
// model file name - (must match [[rust-files]].path for the .smithy file in codegen.toml)
// namespace - must match namespace declaration inside the sample .smithy file
const CREATE_MODEL_SAMPLE: &str = "ping.smithy";
const CREATE_MODEL_NAMESPACE: &str = "org.wasmcloud.example.ping";

#[derive(Clap, Debug)]
#[clap(name = "midl", about, version)]
struct Opt {
    // The number of occurrences of the `v/verbose` flag
    /// Verbose mode (-v, -vv, -vvv, etc.)
    #[clap(short, long, parse(from_occurrences))]
    verbose: u8,

    /// Subcommand
    #[clap(subcommand)]
    command: Command,
}

#[derive(Debug, Clap)]
pub enum Command {
    /// Generate documentation and/or code
    #[clap(name = "gen")]
    Generate(GenerateOpt),

    /// Run lint checks on model
    #[clap(name = "lint")]
    Lint(LintOpt),

    /// Run validation checks on model
    #[clap(name = "validate")]
    Validate(ValidateOpt),

    /// Print json version of model
    #[clap(name = "json")]
    Json(JsonOpt),

    /// Display the cache path or clear the cache
    #[clap(name = "cache")]
    Cache(CacheOpt),

    /// Convert a toml file to json, so it can be used with jq in shell scripts
    #[clap(name = "toml-json")]
    TomlJson(TomlJsonOpt),
}

/// Cache commands. With no args, display the cache path
#[derive(Clap, Debug)]
pub struct CacheOpt {
    /// Clear the entire cache of url models
    #[clap(long)]
    clear_all: bool,
}

#[derive(Clap, Debug)]
pub struct LintOpt {
    /// Configuration file (toml). Defaults to "./codegen.toml".
    /// Only used to get `models` if input files are not specified on the command line.
    #[clap(short, long)]
    config: Option<PathBuf>,

    /// Input files to process
    #[clap(name = "input")]
    input: Vec<String>,
}

#[derive(Clap, Debug)]
pub struct ValidateOpt {
    /// Configuration file (toml). Defaults to "./codegen.toml".
    /// Only used to get `models` if input files are not specified on the command line.
    #[clap(short, long)]
    config: Option<PathBuf>,

    /// Input files to process
    #[clap(name = "input")]
    input: Vec<String>,
}

#[derive(Clap, Debug)]
pub struct JsonOpt {
    /// Configuration file (toml). Defaults to "./codegen.toml".
    /// Only used to get `models` if input files are not specified on the command line.
    #[clap(short, long)]
    config: Option<PathBuf>,

    /// Input files to process. Overrides `[[models` in codegen.toml
    #[clap(name = "input")]
    input: Vec<String>,

    /// whether to  pretty-print json
    #[clap(long)]
    pretty: bool,
}

#[derive(Clap, Debug)]
pub struct GenerateOpt {
    /// Configuration file (toml). Defaults to "./codegen.toml"
    #[clap(short, long)]
    config: Option<PathBuf>,

    /// Output directory, defaults to current directory
    #[clap(short, long)]
    output_dir: Option<PathBuf>,

    /// Optionally, load templates from this folder (overrides built-in templates).
    /// Each template file name is its template name, for example, "header.hbs"
    /// is registered with the name "header"
    #[clap(short = 'T', long)]
    template_dir: Option<PathBuf>,

    /// Flag to create a new project
    #[clap(long)]
    create: Option<String>,

    /// Output language(s) to generate. May be specified more than once
    /// If not specified, all languages in config file will be generated (`-l html -l rust`)
    // number_of_values forces the user to use '-l' for each item
    #[clap(short, long, number_of_values = 1)]
    lang: Vec<OutputLanguage>,

    /// Additional defines in the form of key=value to be passed to renderer
    /// Use `-D key=value` for each term to be added.
    #[clap(short = 'D', parse(try_from_str = parse_key_val), number_of_values = 1)]
    defines: Vec<(String, TomlValue)>,

    /// model files to process. Must specify either on command line or in 'models' array in codegen.toml
    #[clap(name = "input")]
    input: Vec<String>,
}

#[derive(Clap, Debug)]
pub struct TomlJsonOpt {
    /// Toml input file
    #[clap(name = "input")]
    input: PathBuf,
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

fn main() {
    let opt = Opt::parse();

    if let Err(e) = run(opt) {
        eprintln!("Error: {}", e.to_string());
    }
}

fn run(opt: Opt) -> Result<()> {
    match opt.command {
        Command::Generate(gen_opt) => generate(gen_opt, opt.verbose)?,
        Command::Lint(lint_opt) => lint(lint_opt, opt.verbose)?,
        Command::Validate(validate_opt) => validate(validate_opt, opt.verbose)?,
        Command::Json(json_opt) => json(json_opt, opt.verbose)?,
        Command::Cache(cache_opt) => cache(cache_opt)?,
        Command::TomlJson(toml_opt) => toml_json(toml_opt)?,
    }
    Ok(())
}

fn inputs_to_model(inputs: &[String], verbose: u8) -> Result<Model> {
    let inputs = inputs
        .iter()
        // unwrap below ok because this from_str is Infallible
        .map(|s| ModelSource::from_str(s).unwrap())
        .collect::<Vec<ModelSource>>();
    let current_dir = PathBuf::from(".");
    Ok(sources_to_model(&inputs, &current_dir, verbose)?)
}

fn lint(opt: LintOpt, verbose: u8) -> Result<()> {
    use atelier_core::action::lint::{run_linter_actions, NamingConventions, UnwelcomeTerms};

    let config = select_config(&opt.config)?;
    let model = if opt.input.is_empty() {
        sources_to_model(&config.models, &config.base_dir, verbose)?
    } else {
        inputs_to_model(&opt.input, verbose)?
    };
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

    Ok(())
}

fn cache(opt: CacheOpt) -> Result<()> {
    let cache_dir = weld_codegen::weld_cache_dir()?;

    if !opt.clear_all {
        println!("{}", cache_dir.display());
    } else {
        std::fs::remove_dir_all(&cache_dir)?;
        println!("Cache cleared.");
    }
    Ok(())
}

fn json(opt: JsonOpt, verbose: u8) -> Result<()> {
    use std::io::Write;

    let config = select_config(&opt.config)?;
    let model = if opt.input.is_empty() {
        sources_to_model(&config.models, &config.base_dir, verbose)?
    } else {
        inputs_to_model(&opt.input, verbose)?
    };

    let json_ast = atelier_json::model_to_json(&model);
    let text = match opt.pretty {
        true => serde_json::to_string_pretty(&json_ast)?,
        false => serde_json::to_string(&json_ast)?,
    };
    std::io::stdout().write_all(text.as_bytes())?;

    Ok(())
}

fn validate(opt: ValidateOpt, verbose: u8) -> Result<()> {
    use atelier_core::action::validate::{
        run_validation_actions, CorrectTypeReferences, NoUnresolvedReferences,
    };
    let config = select_config(&opt.config)?;
    let model = if opt.input.is_empty() {
        sources_to_model(&config.models, &config.base_dir, verbose)?
    } else {
        inputs_to_model(&opt.input, verbose)?
    };

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

    Ok(())
}

fn generate(mut opt: GenerateOpt, verbose: u8) -> Result<()> {
    if let Some(ref tdir) = opt.template_dir {
        if !tdir.is_dir() {
            return Err(anyhow!(
                "template_dir parameter must be an existing directory"
            ));
        }
    }
    let output_dir = match &opt.output_dir {
        Some(pb) => pb.to_owned(),
        _ => PathBuf::from("."),
    };

    let mut config = select_config(&opt.config)?;
    if !opt.lang.is_empty() {
        config.output_languages = opt.lang.clone()
    }
    if let Some(ref create) = opt.create {
        // Add some initial defaults. Insert at front of list so user defines override.
        // These are used by some of the templates
        opt.defines.insert(
            0,
            ("project_version".to_string(), SAMPLE_PROJECT_VERSION.into()),
        );
        opt.defines
            .insert(0, ("project_name".to_string(), SAMPLE_PROJECT_NAME.into()));
        opt.defines
            .push(("project_create".to_string(), true.into()));

        match create.as_str() {
            "actor" => {
                opt.defines.push(("create_actor".to_string(), true.into()));
            }
            "provider" => {
                opt.defines
                    .push(("create_provider".to_string(), true.into()));
            }
            "interface" => {
                // use last part of namespace for file name
                let ns_last = CREATE_MODEL_NAMESPACE.split('.').last().unwrap();

                opt.defines.extend_from_slice(&[
                    ("create_interface".to_string(), true.into()),
                    (
                        "project_models".to_string(),
                        TomlValue::Array(vec![TomlValue::String(CREATE_MODEL_SAMPLE.to_string())]),
                    ),
                    (
                        "project_namespace".to_string(),
                        CREATE_MODEL_NAMESPACE.into(),
                    ),
                    ("project_namespace_file".to_string(), ns_last.into()),
                ]);
            }
            _ => {
                return Err(anyhow!(
                    "invalid create option. Expected 'actor','provider',or 'interface'"
                ));
            }
        }
    }

    // a parsed model is only needed if we aren't creating a new project
    let model = if opt.create.is_none() {
        if opt.input.is_empty() {
            Some(sources_to_model(&config.models, &config.base_dir, verbose)?)
        } else {
            Some(inputs_to_model(&opt.input, verbose)?)
        }
    } else {
        None
    };

    let templates = if let Some(ref tdir) = opt.template_dir {
        println!("importing templates from {}", &tdir.display());
        templates_from_dir(tdir)?
    } else {
        Vec::new()
    };

    let g = Generator::default();
    g.gen(model.as_ref(), config, templates, &output_dir, opt.defines)?;

    Ok(())
}

/// identify config file from command-line, current-directory, or built-in default
/// Returns the configuration, and whether default was used.
fn select_config(opt_config: &Option<PathBuf>) -> Result<CodegenConfig> {
    // if --config is not specified in the command-line, try the current directory.
    // if it's not found use the default
    let (cfile, folder) = if let Some(path) = &opt_config {
        (
            std::fs::read_to_string(path)
                .map_err(|e| anyhow!("reading config file {}: {}", path.display(), e))?,
            path.parent().unwrap().to_path_buf(),
        )
    } else if PathBuf::from("./codegen.toml").is_file() {
        (
            std::fs::read_to_string("./codegen.toml")
                .map_err(|e| anyhow!("reading config file codegen.toml: {}", e))?,
            PathBuf::from("."),
        )
    } else {
        (DEFAULT_CONFIG.to_string(), PathBuf::from("."))
    };
    let folder = std::fs::canonicalize(folder)?;
    let mut config = cfile.parse::<CodegenConfig>()?;
    config.base_dir = folder;

    Ok(config)
}

/// Convert file from toml to json
fn toml_json(toml_opt: TomlJsonOpt) -> Result<()> {
    use std::io::Write;
    if !toml_opt.input.is_file() {
        return Err(anyhow!("missing file: {}", &toml_opt.input.display()));
    }
    let base_name = toml_opt.input.file_name().unwrap().to_string_lossy();

    let data = std::fs::read_to_string(&toml_opt.input)?;
    let out = if base_name == "Cargo.toml" {
        let manifest = cargo_toml::Manifest::from_str(&data)?;
        serde_json::to_vec(&manifest)?
    } else if base_name == "codegen.toml" {
        let config = CodegenConfig::from_str(&data)?;
        serde_json::to_vec(&config)?
    } else {
        let generic: std::collections::BTreeMap<String, toml::Value> = toml::from_str(&data)?;
        serde_json::to_vec(&generic)?
    };
    std::io::stdout().write(&out)?;
    Ok(())
}
