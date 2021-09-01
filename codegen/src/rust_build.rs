//! The build logic inside build.rs - rebuild model,
//! using instructions in codegen.toml. Emits "rererun-if-changed"
//! instructions to cargo so model is only rebuilt if either codegen.toml
//! or one of the model files changes.

use crate::error::Error;
use std::path::PathBuf;

/// wraps the logic inside build.rs
pub fn rust_build<P: Into<PathBuf>> (
    config_path: P,
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
        .map_err(|e| Error::Build(format!("parsing config: {}", e.to_string())))?;
    config.base_dir = config_path.parent().unwrap().to_path_buf();
    config.output_languages = vec![OutputLanguage::Rust];

    // tell cargo to rebuild if codegen.toml changes
    println!("cargo:rerun-if-changed={}", &config_path.display());

    //let out_dir = std::path::PathBuf::from(&std::env::var("OUT_DIR").unwrap());
    let out_dir = PathBuf::from("."); // current directory

    let model = crate::sources_to_model(&config.models, &config.base_dir, 0)?;

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

    Generator::default().gen(Some(&model), config, Vec::new(), &out_dir, Vec::new())?;

    Ok(())
}
