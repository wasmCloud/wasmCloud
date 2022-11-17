//! Implementations for managing contexts within a directory on a filesystem

use std::fs::File;
use std::io::BufReader;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{ContextManager, DefaultContext, WashContext, HOST_CONFIG_NAME};

const INDEX_JSON: &str = "index.json";

/// A concrete type representing a path to a context directory
pub struct ContextDir(PathBuf);

impl AsRef<Path> for ContextDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Deref for ContextDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ContextDir {
    /// Creates a new ContextDir, erroring if it is unable to access or create the given directory.
    ///
    /// This should be the directory and not the file (e.g. "/home/foo/.wash/contexts")
    pub fn new(path: impl AsRef<Path>) -> Result<ContextDir> {
        let p = path.as_ref();
        let exists = p.exists();
        if exists && !p.is_dir() {
            anyhow::bail!("{} is not a directory (or cannot be accessed)", p.display())
        } else if !exists {
            std::fs::create_dir_all(p)?;
        }
        // Make sure we have the fully qualified path at this point
        Ok(ContextDir(p.canonicalize()?))
    }

    /// Returns a list of paths to all contexts in the context directory
    pub fn list_context_paths(&self) -> Result<Vec<PathBuf>> {
        let paths = std::fs::read_dir(&self.0)?;

        let index = std::ffi::OsString::from(INDEX_JSON);
        Ok(paths
            .filter_map(|p| {
                if let Ok(ctx_entry) = p {
                    let path = ctx_entry.path();
                    let ctx_filename = ctx_entry.file_name();
                    match path.extension().map(|os| os.to_str()).unwrap_or_default() {
                        // Don't include index in the list of contexts
                        Some("json") if ctx_filename == index => None,
                        Some("json") => Some(path),
                        _ => None,
                    }
                } else {
                    None
                }
            })
            .collect())
    }

    /// Returns the full path on disk for the named context
    pub fn get_context_path(&self, name: &str) -> Result<Option<PathBuf>> {
        Ok(self
            .list_context_paths()?
            .into_iter()
            .find(|p| p.file_stem().unwrap_or_default() == name))
    }
}

impl ContextManager for ContextDir {
    /// Returns the name of the currently set default context
    fn default_context(&self) -> Result<String> {
        let raw = match std::fs::read(self.0.join(INDEX_JSON)) {
            Ok(b) => b,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => {
                return Ok(HOST_CONFIG_NAME.to_string())
            }
            Err(e) => return Err(anyhow::Error::from(e)),
        };
        let index: DefaultContext = serde_json::from_slice(&raw)?;
        Ok(index.name.to_owned())
    }

    /// Sets the current default context to the given name. Will error if it doesn't exist
    fn set_default_context(&self, name: &str) -> Result<()> {
        let file = File::create(self.0.join(INDEX_JSON))?;
        if !self
            .list_contexts()
            .map_err(|e| {
                anyhow::anyhow!("Unable to check directory to see if context exists: {}", e)
            })?
            .into_iter()
            .any(|p| p == name)
        {
            anyhow::bail!("Couldn't find context with the name of {}", name)
        }
        serde_json::to_writer(file, &DefaultContext { name }).map_err(anyhow::Error::from)
    }

    /// Saves the given context to the context directory. The file will be named `{ctx.name}.json`
    fn save_context(&self, ctx: &WashContext) -> Result<()> {
        let filepath = context_path_from_name(&self.0, &ctx.name);
        let file = std::fs::File::create(filepath)?;
        serde_json::to_writer(file, ctx).map_err(anyhow::Error::from)
    }

    fn delete_context(&self, name: &str) -> Result<()> {
        let path = context_path_from_name(&self.0, name);
        std::fs::remove_file(path).map_err(anyhow::Error::from)
    }

    /// Loads the currently set default context
    fn load_default_context(&self) -> Result<WashContext> {
        let context = self.default_context()?;
        load_context(context_path_from_name(&self.0, &context))
    }

    /// Loads the named context from disk and deserializes it
    fn load_context(&self, name: &str) -> Result<WashContext> {
        load_context(context_path_from_name(&self.0, name))
    }

    fn list_contexts(&self) -> Result<Vec<String>> {
        Ok(self
            .list_context_paths()?
            .into_iter()
            .filter_map(|p| {
                p.file_stem()
                    .unwrap_or_default()
                    .to_os_string()
                    .into_string()
                    .ok()
            })
            .collect())
    }
}

/// Loads the given file from disk and attempts to deserialize it as a wash context
pub fn load_context(path: impl AsRef<Path>) -> Result<WashContext> {
    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    serde_json::from_reader(reader).map_err(anyhow::Error::from)
}

/// Helper function to properly format the path to a context JSON file
fn context_path_from_name(dir: impl AsRef<Path>, name: &str) -> PathBuf {
    dir.as_ref().join(format!("{}.json", name))
}
