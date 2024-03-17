use std::collections::HashMap;

use anyhow::Result;
use serde_json::json;
use wash_lib::cli::CommandOutput;
use wash_lib::drain::Drain;

pub fn handle_command(cmd: Drain) -> Result<CommandOutput> {
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
