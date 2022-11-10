use crate::{cfg::cfg_dir, up::DOWNLOADS_DIR, util::CommandOutput};
use anyhow::Result;
use clap::Subcommand;
use serde_json::json;
use std::{
    collections::HashMap,
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Subcommand, Debug, Clone)]
pub(crate) enum DrainSelection {
    /// Remove all cached files created by wasmcloud
    All,
    /// Remove cached files downloaded from OCI registries by wasmCloud
    Oci,
    /// Remove cached binaries extracted from provider archives
    Lib,
    /// Remove cached smithy files downloaded from model urls
    Smithy,
    /// Remove downloaded and generated files from launching wasmCloud hosts
    Downloads,
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
                /* Downloads */ downloads_dir(),
            ],
            DrainSelection::Lib => vec![env::temp_dir().join("wasmcloudcache")],
            DrainSelection::Oci => vec![env::temp_dir().join("wasmcloud_ocicache")],
            DrainSelection::Smithy => vec![model_cache_dir()],
            DrainSelection::Downloads => vec![downloads_dir()],
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

fn downloads_dir() -> PathBuf {
    match cfg_dir() {
        Ok(path) => path.join(DOWNLOADS_DIR),
        Err(e) => {
            eprintln!("{}", e);
            "".into()
        }
    }
}

impl DrainSelection {
    fn drain(&self) -> Result<CommandOutput> {
        let cleared = self
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

pub(crate) fn handle_command(cmd: DrainSelection) -> Result<CommandOutput> {
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
    use clap::Parser;

    #[derive(Parser)]
    struct Cmd {
        #[clap(subcommand)]
        drain: DrainSelection,
    }

    #[test]
    // Enumerates all options of drain subcommands to ensure
    // changes are not made to the drain API
    fn test_drain_comprehensive() {
        let all: Cmd = Parser::try_parse_from(&["drain", "all"]).unwrap();
        match all.drain {
            DrainSelection::All => {}
            _ => panic!("drain constructed incorrect command"),
        }
        let lib: Cmd = Parser::try_parse_from(&["drain", "lib"]).unwrap();
        match lib.drain {
            DrainSelection::Lib => {}
            _ => panic!("drain constructed incorrect command"),
        }
        let oci: Cmd = Parser::try_parse_from(&["drain", "oci"]).unwrap();
        match oci.drain {
            DrainSelection::Oci => {}
            _ => panic!("drain constructed incorrect command"),
        }
        let smithy: Cmd = Parser::try_parse_from(&["drain", "smithy"]).unwrap();
        match smithy.drain {
            DrainSelection::Smithy => {}
            _ => panic!("drain constructed incorrect command"),
        }
    }
}
