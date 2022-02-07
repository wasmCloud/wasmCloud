//! The build logic inside build.rs - rebuild model,
//! using instructions in codegen.toml. Emits "rererun-if-changed"
//! instructions to cargo so model is only rebuilt if either codegen.toml
//! or one of the model files changes.

use crate::error::Error;
use std::path::PathBuf;

/// wraps the logic inside build.rs
pub fn rust_build<P: Into<PathBuf>>(
    config_path: P,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    rust_build_into(config_path, ".")
}

/// Generate rust code using the codegen.toml config file and output directory.
/// `config_path` should be relative to the directory containing build.rs (or absolute).
/// `output_relative_dir`, the location of generated files, should either be an absolute path,
/// or a path relative to the folder containing the codegen.toml file.
/// To use rustc's build folder, from inside build.rs, use `&std::env::var("OUT_DIR").unwrap()`
pub fn rust_build_into<CFG: Into<PathBuf>, OUT: Into<PathBuf>>(
    config_path: CFG,
    output_relative_dir: OUT,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use crate::{
        config::{CodegenConfig, OutputLanguage},
        Generator,
    };
    let config_path = config_path.into();
    if !config_path.is_file() {
        return Err(Error::Build(format!("missing config file {}", &config_path.display())).into());
    }
    let config_path = std::fs::canonicalize(config_path)?;
    let config_file = std::fs::read_to_string(&config_path).map_err(|e| {
        Error::Build(format!(
            "error reading config file '{}': {}",
            &config_path.display(),
            e
        ))
    })?;
    let mut config = config_file
        .parse::<CodegenConfig>()
        .map_err(|e| Error::Build(format!("parsing config: {}", e)))?;
    config.base_dir = config_path.parent().unwrap().to_path_buf();
    config.output_languages = vec![OutputLanguage::Rust];

    // tell cargo to rebuild if codegen.toml changes
    println!("cargo:rerun-if-changed={}", &config_path.display());

    let model = crate::sources_to_model(&config.models, &config.base_dir, 0)?;

    if let Err(msgs) = crate::validate::validate(&model) {
        return Err(Box::new(Error::Build(msgs.join("\n"))));
    }

    // the second time we do this it should be faster since no downloading is required,
    // and we also don't invoke assembler to traverse directories
    for path in crate::sources_to_paths(&config.models, &config.base_dir, 0)?.into_iter() {
        // rerun-if-changed works on directories and files, so it's ok that sources_to_paths
        // may include folders that haven't been traversed by the assembler.
        // Using a folder depends on the OS updating folder mtime if the folder contents change.
        // In many cases, the model file for the primary interface/namespace will
        // be a file path (it is in projects created with `weld create`).
        // All paths returned from sources_to_paths are absolute (by joining to config.base_dir)
        // so we don't need to adjust them here
        if path.exists() {
            println!("cargo:rerun-if-changed={}", &path.display());
        }
    }

    Generator::default().gen(
        Some(&model),
        config,
        Vec::new(),
        &output_relative_dir.into(),
        Vec::new(),
    )?;

    Ok(())
}
