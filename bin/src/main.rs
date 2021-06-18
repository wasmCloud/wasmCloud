use anyhow::{anyhow, Result};
use atelier_core::io::ModelWriter;
use bytes::BytesMut;
use clap::{self, Clap};
use serde_json::Value;
use std::{collections::BTreeSet, path::PathBuf, string::ToString};
use wasmcloud_weld_codegen::{self as codegen, CodeGen};
use wasmcloud_weld_docgen::render::{add_templates_from_dir, RenderConfig, RenderError, Renderer};

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
    /// Smithy code generation
    #[clap(name = "gen")]
    Generate(GenerateOpt),

    /// Document smithy model
    #[clap(name = "doc")]
    Document(DocumentationOpt),

    /// run lint checks on model
    #[clap(name = "lint")]
    Lint(LintOpt),

    /// run validation checks on model
    #[clap(name = "validate")]
    Validate(ValidateOpt),
}

#[derive(Clap, Debug)]
pub struct GenerateOpt {
    /// Output file
    #[clap(short, long)]
    output: PathBuf,

    /// Input files to process
    #[clap(short, long)]
    input: Vec<PathBuf>,
}

#[derive(Clap, Debug)]
pub struct LintOpt {
    /// Input files to process
    #[clap(short, long)]
    input: Vec<PathBuf>,
}

#[derive(Clap, Debug)]
pub struct ValidateOpt {
    /// Input files to process
    #[clap(short, long)]
    input: Vec<PathBuf>,
}

#[derive(Clap, Debug)]
pub struct DocumentationOpt {
    #[clap(short = 'd', long)]
    output_dir: PathBuf,

    /// Single output file with all namespaces in one html file.
    /// Overrides output directory
    #[clap(short, long)]
    single: Option<PathBuf>,

    /// Input smithy files to process
    #[clap(short, long)]
    input: Vec<PathBuf>,

    // optional path for saving intermediate json
    //#[clap(short, long)]
    //json: Option<PathBuf>,
    /// Optionally, load templates from this folder (overrides built-in templates).
    /// Each template file name is its template name, for example, "header.hbs"
    /// is registered with the name "header"
    #[clap(short = 'T', long)]
    template_dir: Option<PathBuf>,

    /// name for starting template, default ="namespace"
    #[clap(short, long, default_value = "namespace")]
    template: String,
}

fn main() {
    let opt = Opt::parse();
    if opt.verbose > 2 {
        println!("{:#?}", &opt);
    }

    if let Err(e) = run(opt) {
        eprintln!("Error: {}", e.to_string());
    }
}

fn run(opt: Opt) -> Result<()> {
    match &opt.command {
        Command::Generate(gen_opt) => generate(gen_opt)?,
        Command::Document(doc_opt) => document(doc_opt)?,
        Command::Lint(lint_opt) => lint(lint_opt)?,
        Command::Validate(validate_opt) => validate(validate_opt)?,
    }
    Ok(())
}

fn generate(opt: &GenerateOpt) -> Result<()> {
    // load all input files specified
    let model = codegen::load_model(&opt.input)?;

    let mut gen = codegen::codegen_rust::RustCodeGen::default();
    let bytes = gen.codegen(&model)?;

    std::fs::write(&opt.output, &bytes)?;

    let rustfmt = codegen::rustfmt::RustFmtCommand::default();
    rustfmt.execute(vec![&opt.output])?;
    Ok(())
}

fn lint(opt: &LintOpt) -> Result<()> {
    use atelier_core::action::lint::{run_linter_actions, NamingConventions, UnwelcomeTerms};
    //use atelier_core::action::Linter;
    // load all input files specified, generate model in json
    let model = codegen::load_model(&opt.input)?;
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

fn validate(opt: &ValidateOpt) -> Result<()> {
    use atelier_core::action::validate::{
        run_validation_actions, CorrectTypeReferences, NoUnresolvedReferences,
    };
    //use atelier_core::action::Validator;
    // load all input files specified, generate model in json
    let model = codegen::load_model(&opt.input)?;
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

fn document(opt: &DocumentationOpt) -> Result<()> {
    use bytes::BufMut;
    if let Some(ref tdir) = opt.template_dir {
        if !tdir.is_dir() {
            return Err(
                RenderError::new("template_dir parameter must be an existing directory").into(),
            );
        }
    }
    if !opt.output_dir.is_dir() {
        return Err(RenderError::new("output_dir must be an existing directory").into());
    }

    // load all input files specified, generate model in json
    let model = codegen::load_model(&opt.input)?;

    // Get list of namespaces. model.namespaces() is not unique, so use a BTreeSet
    // to make it sorted and unique, then make it a Vec<Value> to pass to handlebars
    let namespaces = model
        .namespaces()
        .iter()
        .map(|id| id.to_string())
        .collect::<BTreeSet<String>>();

    let mut renderer = Renderer::init(&RenderConfig::default())?;
    if let Some(template_dir) = &opt.template_dir {
        add_templates_from_dir(template_dir, &mut renderer)?;
    }

    let mut writer = BytesMut::default().writer();
    let mut json_writer = atelier_json::JsonWriter::default();
    json_writer
        .write(&mut writer, &model)
        .map_err(|e| RenderError::new(&format!("write error {}", e)))?;
    let model_json = serde_json::from_slice(&writer.into_inner())?;
    renderer.set("model", Value::Object(model_json));

    for ns in namespaces.iter() {
        let output_file = opt
            .output_dir
            .join(format!("{}.html", codegen::strings::to_snake_case(ns)));
        let mut out = std::fs::File::create(output_file)?;
        renderer.set("namespace", Value::String(ns.clone()));
        renderer.set("minified", Value::Bool(true));
        renderer.set("title", Value::String(ns.clone()));
        renderer.render(&opt.template, &mut out)?;
    }
    Ok(())
}
