// stripped-down no-options config generator. `wash gen` has more options.
// The purpose of this is to test new builds of codegen without needing to rebuild wash.
// Output is relative to current dir (language out_dir in codegen.toml can use absolute paths)

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use weld_codegen::{config::CodegenConfig, sources_to_model};

const VERBOSITY: u8 = 1; // none=0, +1 for higher

fn main() -> Result<()> {
    let config_path = std::env::args().nth(1).unwrap_or_else(|| "./codegen.toml".to_string());
    let config_path = std::fs::canonicalize(PathBuf::from(&config_path))?;
    let config = load_config(&config_path)
        .with_context(|| format!("error loading config at {}", config_path.display()))?;
    gen(config)?;
    Ok(())
}

fn load_config(path: &Path) -> Result<CodegenConfig> {
    let cfile = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file at {}", &path.display()))?;
    let folder = path.parent().unwrap().to_path_buf();
    let folder = std::fs::canonicalize(folder)?;

    let mut config = cfile.parse::<CodegenConfig>()?;
    config.base_dir = folder;
    Ok(config)
}

fn gen(mut config: CodegenConfig) -> Result<()> {
    let mut input_models = Vec::new();
    std::mem::swap(&mut config.models, &mut input_models);
    let model = sources_to_model(&input_models, &config.base_dir.clone(), VERBOSITY)?;
    let g = weld_codegen::Generator::default();
    g.gen(
        Some(&model),
        config,
        Vec::new(),
        &PathBuf::from("."),
        Vec::new(),
    )?;
    Ok(())
}
