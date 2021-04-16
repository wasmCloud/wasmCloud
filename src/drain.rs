use crate::util::{Output, OutputKind};
use serde_json::json;
use std::env;
use std::path::Path;
use std::{fs, path::PathBuf};
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

// Propagates output selection from CLI to all commands
impl DrainCliCommand {
    fn output_kind(&self) -> OutputKind {
        match self.selection {
            DrainSelection::All(output)
            | DrainSelection::Lib(output)
            | DrainSelection::Oci(output) => output.kind,
        }
    }
}

#[derive(StructOpt, Debug, Clone)]
pub(crate) enum DrainSelection {
    /// Remove all cached files created by wasmcloud
    All(Output),
    /// Remove cached files downloaded from OCI registries by wasmcloud
    Oci(Output),
    /// Remove cached binaries extracted from provider archives
    Lib(Output),
}

impl IntoIterator for &DrainSelection {
    type Item = PathBuf;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        let paths = match self {
            DrainSelection::All(_) => vec![
                env::temp_dir().join("wasmcloudcache"),
                env::temp_dir().join("wasmcloud_ocicache"),
            ],
            DrainSelection::Oci(_) => vec![env::temp_dir().join("wasmcloud_ocicache")],
            DrainSelection::Lib(_) => vec![env::temp_dir().join("wasmcloudcache")],
        };
        paths.into_iter()
    }
}

impl DrainCliCommand {
    fn drain(&self) -> Result<String, Box<dyn ::std::error::Error>> {
        let to_clear = self.selection.into_iter();
        let mut cleared = vec![];
        for path in to_clear {
            cleared.push(remove_dir_contents(path)?);
        }
        Ok(match self.output_kind() {
            OutputKind::Text => format!("Successfully cleared caches at: {:?}", cleared),
            OutputKind::Json => json!({ "drained": cleared }).to_string(),
        })
    }
}

pub(crate) fn handle_command(cmd: DrainCliCommand) -> Result<String, Box<dyn ::std::error::Error>> {
    cmd.drain()
}

fn remove_dir_contents<P: AsRef<Path>>(path: P) -> Result<String, Box<dyn ::std::error::Error>> {
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
        let all = DrainCli::from_iter_safe(&["drain", "all", "-o", "text"]).unwrap();
        match all.command.selection {
            DrainSelection::All(output) => assert_eq!(output.kind, OutputKind::Text),
            _ => panic!("drain constructed incorrect command"),
        }
        let lib = DrainCli::from_iter_safe(&["drain", "lib", "-o", "text"]).unwrap();
        match lib.command.selection {
            DrainSelection::Lib(output) => assert_eq!(output.kind, OutputKind::Text),
            _ => panic!("drain constructed incorrect command"),
        }
        let oci = DrainCli::from_iter_safe(&["drain", "oci", "-o", "json"]).unwrap();
        match oci.command.selection {
            DrainSelection::Oci(output) => assert_eq!(output.kind, OutputKind::Json),
            _ => panic!("drain constructed incorrect command"),
        }
    }
}
