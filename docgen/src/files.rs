use std::path::{Path, PathBuf};

/// Search a folder recursively for files ending with the provided extension
/// Filenames must be utf-8 characters
pub fn find_files(dir: &Path, extension: &str) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut results = Vec::new();
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                results.append(&mut find_files(&path, extension)?);
            } else {
                let ext = path
                    .extension()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if ext == extension {
                    results.push(path)
                }
            }
        }
    }
    Ok(results)
}
