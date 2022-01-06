use crate::util::CommandOutput;
use anyhow::Result;
use serde_json::json;
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};
use structopt::StructOpt;

#[derive(Debug, Clone, StructOpt)]
pub(crate) struct DrainCli {
    #[structopt(flatten)]
    command: DrainCliCommand,
}

impl DrainCli {
    pub(crate) fn command(self) -> DrainCliCommand {
        self.command
    }
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) struct DrainCliCommand {
    #[structopt(subcommand)]
    selection: DrainSelection,
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) enum DrainSelection {
    /// Remove all cached files created by wasmcloud
    All,
    /// Remove cached files downloaded from OCI registries by wasmcloud
    Oci,
    /// Remove cached binaries extracted from provider archives
    Lib,
    /// Remove cached smithy files downloaded from model urls
    Smithy,
}

impl IntoIterator for &DrainSelection {
    type Item = PathBuf;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        let paths = match self {
            DrainSelection::All => vec![
                /* Lib    */ env::temp_dir().join("wasmcloudcache"),
                /* Oci    */ env::temp_dir().join("wasmcloud_ocicache"),
                /* Smithy */ model_cache_dir(),
            ],
            DrainSelection::Lib => vec![env::temp_dir().join("wasmcloudcache")],
            DrainSelection::Oci => vec![env::temp_dir().join("wasmcloud_ocicache")],
            DrainSelection::Smithy => vec![model_cache_dir()],
        };
        paths.into_iter()
    }
}

fn model_cache_dir() -> PathBuf {
    match weld_codegen::weld_cache_dir() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("{}", e);
            "".into()
        }
    }
}

impl DrainCliCommand {
    fn drain(&self) -> Result<CommandOutput> {
        let cleared = self
            .selection
            .into_iter()
            .filter(|path| path.exists())
            .map(remove_dir_contents)
            .collect::<Result<Vec<String>>>()?;

        let mut map = HashMap::new();
        map.insert("drained".to_string(), json!(cleared));
        Ok(CommandOutput::new(
            format!("Successfully cleared caches at: {:?}", cleared),
            map,
        ))
    }
}

pub(crate) fn handle_command(cmd: DrainCliCommand) -> Result<CommandOutput> {
    cmd.drain()
}

fn remove_dir_contents<P: AsRef<Path>>(path: P) -> Result<String> {
    for entry in fs::read_dir(&path)? {
        let path = entry?.path();
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    Ok(format!("{}", path.as_ref().display()))
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    // Enumerates all options of drain subcommands to ensure
    // changes are not made to the drain API
    fn test_drain_comprehensive() {
        let all = DrainCli::from_iter_safe(&["drain", "all"]).unwrap();
        match all.command.selection {
            DrainSelection::All => {}
            _ => panic!("drain constructed incorrect command"),
        }
        let lib = DrainCli::from_iter_safe(&["drain", "lib"]).unwrap();
        match lib.command.selection {
            DrainSelection::Lib => {}
            _ => panic!("drain constructed incorrect command"),
        }
        let oci = DrainCli::from_iter_safe(&["drain", "oci"]).unwrap();
        match oci.command.selection {
            DrainSelection::Oci => {}
            _ => panic!("drain constructed incorrect command"),
        }
        let smithy = DrainCli::from_iter_safe(&["drain", "smithy"]).unwrap();
        match smithy.command.selection {
            DrainSelection::Smithy => {}
            _ => panic!("drain constructed incorrect command"),
        }
    }
}
