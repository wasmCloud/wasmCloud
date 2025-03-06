//! Implementations for managing contexts within a directory on a filesystem

use std::fs::File;
use std::io::BufReader;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::lib::config::{cfg_dir, DEFAULT_CTX_DIR_NAME};

use super::{ContextManager, WashContext, HOST_CONFIG_NAME};

const DEFAULT: &str = "default";

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
    /// Creates and initializes a new `ContextDir` at ~/.wash/contexts
    pub fn new() -> Result<Self> {
        Self::from_dir(None::<&Path>)
    }

    /// Creates and initializes a new [`ContextDir`] at the specified path. If a path is not provided, defaults to ~/.wash/contexts
    pub fn from_dir(path: Option<impl AsRef<Path>>) -> Result<Self> {
        let path = if let Some(path) = path {
            path.as_ref().to_path_buf()
        } else {
            default_context_dir()?
        };

        let exists = path.exists();
        if exists && !path.is_dir() {
            anyhow::bail!(
                "{} is not a directory (or cannot be accessed)",
                path.display()
            )
        } else if !exists {
            std::fs::create_dir_all(&path).context("failed to create context directory")?;
        }

        // Make sure we have the fully qualified path at this point
        let context_dir = path
            .canonicalize()
            .context("failed to canonicalize context directory path")?;

        // Initialize the default context if it doesn't exist
        let default_path = context_dir.join(DEFAULT);
        if !default_path.exists() {
            initialize_context_dir(&context_dir, &default_path)?;
        }

        Ok(Self(context_dir))
    }

    /// Returns a list of paths to all contexts in the context directory
    pub fn list_context_paths(&self) -> Result<Vec<PathBuf>> {
        let entries = std::fs::read_dir(&self.0)?;

        let paths = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| {
                path.extension()
                    .and_then(|os| os.to_str())
                    .unwrap_or_default()
                    == "json"
            })
            // Filter old index.json files. TODO: remove me after a few releases
            .filter(|path| {
                path.file_stem()
                    .and_then(|os| os.to_str())
                    .unwrap_or_default()
                    != "index"
            })
            .collect();
        Ok(paths)
    }

    /// Returns the full path on disk for the named context
    pub fn get_context_path(&self, name: &str) -> Result<Option<PathBuf>> {
        Ok(self
            .list_context_paths()?
            .into_iter()
            .find(|p| p.file_stem().unwrap_or_default() == name))
    }
}

fn default_context_dir() -> Result<PathBuf> {
    Ok(cfg_dir()?.join(DEFAULT_CTX_DIR_NAME))
}

fn initialize_context_dir(context_dir: &Path, default_path: &PathBuf) -> Result<()> {
    let mut default_context_name = HOST_CONFIG_NAME.to_string();

    // TEMPORARY (TM): look for and parse existing index.json, to preserve backwards compatibility
    if let Ok(index_file) = File::open(context_dir.join("index.json")) {
        #[derive(serde::Deserialize)]
        struct DefaultContext {
            name: String,
        }

        if let Ok(old_default_context) =
            serde_json::from_reader::<_, DefaultContext>(BufReader::new(index_file))
        {
            default_context_name = old_default_context.name;
        }
    }
    // END TEMPORARY (TM)

    std::fs::write(default_path, default_context_name.as_bytes()).with_context(|| {
        format!(
            "failed to write default context to `{}`",
            default_path.display(),
        )
    })?;

    let host_config_path = context_dir.join(format!("{default_context_name}.json"));
    if !host_config_path.exists() {
        let host_config_context = WashContext::named(default_context_name);
        std::fs::write(
            &host_config_path,
            serde_json::to_vec(&host_config_context)
                .context("failed to serialize host_config context")?,
        )
        .with_context(|| {
            format!(
                "failed to write host_config context to `{}`",
                host_config_path.display()
            )
        })?;
    }

    Ok(())
}

impl ContextManager for ContextDir {
    /// Returns the name of the currently set default context
    fn default_context_name(&self) -> Result<String> {
        let raw = std::fs::read(self.0.join(DEFAULT)).context("failed to read default context")?;
        let name = std::str::from_utf8(&raw).context("failed to read default context")?;
        Ok(name.to_string())
    }

    /// Sets the current default context to the given name
    fn set_default_context(&self, name: &str) -> Result<()> {
        self.load_context(name).context("context does not exist")?;

        let default_path = self.0.join(DEFAULT);
        std::fs::write(&default_path, name.as_bytes()).with_context(|| {
            format!(
                "failed to write default context to `{}`",
                default_path.display()
            )
        })
    }

    /// Saves the given context to the context directory. The file will be named `{ctx.name}.json`
    fn save_context(&self, ctx: &WashContext) -> Result<()> {
        let filepath = context_path_from_name(&self.0, &ctx.name);
        std::fs::write(
            &filepath,
            serde_json::to_vec(&ctx).context("failed to serialize context")?,
        )
        .with_context(|| {
            format!(
                "failed to save context `{}` to `{}`",
                ctx.name,
                filepath.display()
            )
        })
    }

    fn delete_context(&self, name: &str) -> Result<()> {
        let path = context_path_from_name(&self.0, name);
        std::fs::remove_file(path).context("failed to remove context")?;
        if self.default_context_name()? == name {
            self.set_default_context(HOST_CONFIG_NAME)?; // reset default
        }
        Ok(())
    }

    /// Loads the currently set default context
    fn load_default_context(&self) -> Result<WashContext> {
        self.load_context(&self.default_context_name()?)
    }

    /// Loads the named context from disk
    fn load_context(&self, name: &str) -> Result<WashContext> {
        let path = context_path_from_name(&self.0, name);
        let file = std::fs::File::open(&path)
            .with_context(|| format!("failed to open context file [{}]", path.display()))?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).context("failed to parse context")
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

/// Helper function to properly format the path to a context JSON file
fn context_path_from_name(dir: impl AsRef<Path>, name: &str) -> PathBuf {
    dir.as_ref().join(format!("{name}.json"))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn round_trip_happy_path() {
        let tempdir = tempfile::tempdir().expect("Unable to create tempdir");
        let contexts_path = tempdir.path().join("contexts");
        let ctx_dir = ContextDir::from_dir(Some(&contexts_path))
            .expect("Should be able to create context dir");

        assert!(
            contexts_path.exists() && contexts_path.is_dir(),
            "Non-existent directory should have been created"
        );

        let mut orig_ctx = WashContext {
            name: "happy_path".to_string(),
            lattice: "foobar".to_string(),
            ..Default::default()
        };

        ctx_dir
            .save_context(&orig_ctx)
            .expect("Should be able to save a context to disk");

        let filenames: std::collections::HashSet<String> = contexts_path
            .read_dir()
            .unwrap()
            .filter_map(|entry| entry.unwrap().file_name().into_string().ok())
            .collect();
        let expected_filenames = std::collections::HashSet::from([
            "default".to_string(),
            "host_config.json".to_string(),
            "happy_path.json".to_string(),
        ]);

        assert_eq!(
            filenames, expected_filenames,
            "Newly created context should exist"
        );

        // Now load the context from disk and compare
        let loaded = ctx_dir
            .load_context("happy_path")
            .expect("Should be able to load context from disk");
        assert!(
            orig_ctx.name == loaded.name && orig_ctx.lattice == loaded.lattice,
            "Should have loaded the correct context from disk"
        );

        // Save one more context
        orig_ctx.name = "happy_gilmore".to_string();
        orig_ctx.lattice = "baz".to_string();
        ctx_dir
            .save_context(&orig_ctx)
            .expect("Should be able to save second context");

        assert_eq!(
            contexts_path.read_dir().unwrap().count(),
            4,
            "Directory should have 4 entries"
        );

        ctx_dir
            .set_default_context("happy_gilmore")
            .expect("Should be able to set default context");
        assert_eq!(
            ctx_dir
                .default_context_name()
                .expect("Should be able to load default context"),
            "happy_gilmore",
            "Default context should be correct"
        );

        // Load the default context
        let loaded = ctx_dir
            .load_default_context()
            .expect("Should be able to load default context from disk");
        assert!(
            orig_ctx.name == loaded.name && orig_ctx.lattice == loaded.lattice,
            "Should have loaded the correct context from disk"
        );

        assert_eq!(
            contexts_path.read_dir().unwrap().count(),
            4,
            "Directory should have a new entry from the default context"
        );

        assert!(
            contexts_path.join("default").exists(),
            "default file should exist in directory after setting default context"
        );

        // List the contexts
        let list = ctx_dir
            .list_contexts()
            .expect("Should be able to list contexts");
        assert_eq!(list.len(), 3, "Should only list 3 contexts");
        for ctx in list {
            assert!(
                ctx == "happy_path" || ctx == "happy_gilmore" || ctx == "host_config",
                "Should have found only the contexts we created"
            );
        }

        ctx_dir
            .set_default_context("happy_path")
            .expect("Should be able to set default context");

        assert_eq!(
            ctx_dir
                .default_context_name()
                .expect("Should be able to load default context"),
            "happy_path",
            "Default context should be correct"
        );

        // Delete a context
        ctx_dir
            .delete_context("happy_path")
            .expect("Should be able to delete context");

        assert!(
            !contexts_path.read_dir().unwrap().any(|p| p
                .unwrap()
                .path()
                .as_os_str()
                .to_str()
                .unwrap()
                .contains("happy_path")),
            "Context should have been removed from directory"
        );
    }

    #[test]
    fn load_non_existent_contexts() {
        let tempdir = tempfile::tempdir().expect("Unable to create tempdir");
        let ctx_dir =
            ContextDir::from_dir(Some(&tempdir)).expect("Should be able to create context dir");

        ctx_dir
            .load_default_context()
            .expect("The default context should be automatically created");

        ctx_dir
            .load_context("idontexist")
            .expect_err("Loading a non-existent context should error");
    }

    #[test]
    fn default_context_with_no_settings() {
        let tempdir = tempfile::tempdir().expect("Unable to create tempdir");
        let ctx_dir =
            ContextDir::from_dir(Some(&tempdir)).expect("Should be able to create context dir");

        assert_eq!(
            ctx_dir
                .default_context_name()
                .expect("Should be able to get a default context with nothing set"),
            "host_config",
            "Unset context should return none",
        );

        ctx_dir
            .set_default_context("idontexist")
            .expect_err("Should not be able to set a default context that doesn't exist");
    }

    const PRE_REFACTOR_CONTEXT: &str = r#"{"name":"host_config","cluster_seed":"SCAJ3HQZCDA562YW3VUHHIAUJ2SUCYUNGDCP5DBKQOTEZ6ZZGBKT5NI3DQ","ctl_host":"127.0.0.1","ctl_port":5893,"ctl_jwt":"","ctl_seed":"","ctl_credsfile":null,"ctl_timeout":2000,"ctl_lattice_prefix":"default","rpc_host":"127.0.0.1","rpc_port":5893,"rpc_jwt":"","rpc_seed":"","rpc_credsfile":null,"rpc_timeout":2000,"rpc_lattice_prefix":"default"}"#;

    #[test]
    fn works_with_existing() {
        let tempdir = tempfile::tempdir().expect("Unable to create tempdir");
        std::fs::write(
            tempdir.path().join("host_config.json"),
            PRE_REFACTOR_CONTEXT,
        )
        .expect("Unable to write test data to disk");
        let ctx_dir =
            ContextDir::from_dir(Some(&tempdir)).expect("Should be able to create context dir");

        let ctx = ctx_dir
            .load_context("host_config")
            .expect("Should be able to load a pre-existing context");

        assert!(
            ctx.name == "host_config" && ctx.ctl_port == 5893,
            "Should read the correct data from disk"
        );
    }

    #[test]
    fn delete_default_context() {
        let tempdir = tempfile::tempdir().expect("Unable to create tempdir");
        let ctx_dir =
            ContextDir::from_dir(Some(&tempdir)).expect("Should be able to create context dir");

        let mut ctx = WashContext {
            name: "deleteme".to_string(),
            ..Default::default()
        };

        ctx_dir
            .save_context(&ctx)
            .expect("Should be able to save a context to disk");
        ctx.name = "keepme".to_string();
        ctx_dir
            .save_context(&ctx)
            .expect("Should be able to save a context to disk");

        ctx_dir
            .set_default_context("deleteme")
            .expect("Should be able to set default context");

        assert_eq!(
            tempdir.path().read_dir().unwrap().count(),
            4,
            "Directory should have 4 entries"
        );

        ctx_dir
            .delete_context("deleteme")
            .expect("Should be able to delete context");

        assert_eq!(
            tempdir.path().read_dir().unwrap().count(),
            3,
            "Directory should have 3 entries"
        );

        assert_eq!(
            ctx_dir
                .default_context_name()
                .expect("Should be able to get default context"),
            "host_config",
            "default context should be reset"
        );
    }
}
