//! Project file discovery shared by wash and the offline fixture check.

use std::path::{Path, PathBuf};

pub const CONFIG_FILE_NAME: &str = "config.yaml";
pub const CONFIG_DIR_NAME: &str = ".wash";
pub const VALID_CONFIG_FILES: [&str; 4] =
    ["config.yaml", "config.yml", "config.json", "config.toml"];

pub fn locate_project_config(project_dir: &Path) -> PathBuf {
    locate_config(&project_dir.join(CONFIG_DIR_NAME))
}

pub fn locate_config(dir: &Path) -> PathBuf {
    VALID_CONFIG_FILES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.exists())
        .unwrap_or_else(|| dir.join(CONFIG_FILE_NAME))
}

/// Search the source itself, its children, then its deps. The first location
/// declaring a given package wins, so preserve this order in both resolvers.
pub(crate) fn namespace_candidates(source: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![source.to_path_buf()];
    for dir in [source.to_path_buf(), source.join("deps")] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<_> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir() || path.extension().is_some_and(|ext| ext == "wit"))
            .collect();
        paths.sort();
        candidates.extend(paths);
    }
    candidates
}
