use std::collections::HashMap;

use crate::lib::cli::CommandOutput;
use crate::lib::config::{host_pid_file, wadm_pid_file};
use crate::lib::drain::Drain;
use anyhow::Result;
use serde_json::json;

pub fn handle_command(cmd: Drain) -> Result<CommandOutput, anyhow::Error> {
    if matches!(cmd, Drain::All | Drain::Downloads) {
        let wasmcloud_pid_path = host_pid_file().unwrap();
        let wadm_pid_path = wadm_pid_file().unwrap();

        if wasmcloud_pid_path.exists() || wadm_pid_path.exists() {
            anyhow::bail!("Cannot clear the caches : wasmcloud and wadm PID files are still present on disk, processes might still be active or were not properly terminated.")
        }
    }
    let paths = cmd.drain()?;
    let mut map = HashMap::new();
    map.insert("drained".to_string(), json!(paths));
    Ok(CommandOutput::new(
        format!("Successfully cleared caches at: {paths:?}"),
        map,
    ))
}

#[cfg(test)]
mod test {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cmd {
        #[clap(subcommand)]
        drain: Drain,
    }

    #[test]
    // Enumerates all options of drain subcommands to ensure
    // changes are not made to the drain API
    fn test_drain_comprehensive() {
        let all: Cmd = Parser::try_parse_from(["drain", "all"]).unwrap();
        match all.drain {
            Drain::All => {}
            _ => panic!("drain constructed incorrect command"),
        }
        let lib: Cmd = Parser::try_parse_from(["drain", "lib"]).unwrap();
        match lib.drain {
            Drain::Lib => {}
            _ => panic!("drain constructed incorrect command"),
        }
        let oci: Cmd = Parser::try_parse_from(["drain", "oci"]).unwrap();
        match oci.drain {
            Drain::Oci => {}
            _ => panic!("drain constructed incorrect command"),
        }
    }
}
