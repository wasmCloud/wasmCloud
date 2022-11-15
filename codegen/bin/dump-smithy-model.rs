//! dump model in json

use anyhow::{anyhow, Context, Result};
use clap::{self, Parser};
use std::path::PathBuf;
use weld_codegen::{config::CodegenConfig, sources_to_model};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// codegen.toml file (default: "./codegen.toml")
    #[arg(short, long)]
    config: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config_path = args.config.unwrap_or_else(|| PathBuf::from("./codegen.toml"));
    if !config_path.is_file() {
        return Err(anyhow!("missing config file {}", &config_path.display()));
    }
    let config = std::fs::read_to_string(&config_path)
        .with_context(|| format!("error reading config file {}", config_path.display()))?
        .parse::<CodegenConfig>()?;
    let base_dir = config_path.parent().unwrap().to_path_buf();
    let model = sources_to_model(&config.models, &base_dir, 0)?;
    let json_model = atelier_json::model_to_json(&model);

    let out = std::io::stdout();
    serde_json::to_writer(&out, &json_model)?;
    Ok(())
}
