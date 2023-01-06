use anyhow::{Context, Result};
use std::{
    env,
    fs::{create_dir_all, remove_dir_all},
    path::PathBuf,
};

#[allow(unused)]
pub(crate) const LOCAL_REGISTRY: &str = "localhost:5001";

/// Helper function to create the `wash` binary process
#[allow(unused)]
pub(crate) fn wash() -> std::process::Command {
    test_bin::get_test_bin("wash")
}

#[allow(unused)]
pub(crate) fn output_to_string(output: std::process::Output) -> Result<String> {
    String::from_utf8(output.stdout).with_context(|| "Failed to convert output bytes to String")
}

#[allow(unused)]
pub(crate) fn get_json_output(output: std::process::Output) -> Result<serde_json::Value> {
    let output_str = output_to_string(output)?;

    let json: serde_json::Value = serde_json::from_str(&output_str)
        .with_context(|| "Failed to parse json from output string")?;

    Ok(json)
}

#[allow(unused)]
/// Creates a subfolder in the test directory for use with a specific test
/// It's preferred that the same test that calls this function also
/// uses std::fs::remove_dir_all to remove the subdirectory
pub(crate) fn test_dir_with_subfolder(subfolder: &str) -> PathBuf {
    let root_dir = &env::var("CARGO_MANIFEST_DIR").expect("$CARGO_MANIFEST_DIR");
    let with_subfolder = PathBuf::from(format!("{}/tests/fixtures/{}", root_dir, subfolder));
    remove_dir_all(with_subfolder.clone());
    create_dir_all(with_subfolder.clone());
    with_subfolder
}

#[allow(unused)]
/// Returns a PathBuf by appending the subfolder and file arguments
/// to the test fixtures directory. This does _not_ create the file,
/// so the test is responsible for initialization and modification of this file
pub(crate) fn test_dir_file(subfolder: &str, file: &str) -> PathBuf {
    let root_dir = &env::var("CARGO_MANIFEST_DIR").expect("$CARGO_MANIFEST_DIR");
    PathBuf::from(format!(
        "{}/tests/fixtures/{}/{}",
        root_dir, subfolder, file
    ))
}
