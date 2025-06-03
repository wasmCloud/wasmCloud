//! Functionality enabling the `wash link` group of subcommands

use anyhow::Result;
use crate::lib::cli::link::LinkCommand;
use crate::lib::cli::{CommandOutput, OutputKind};

mod del;
mod put;
mod query;

/// Invoke `wash link` subcommand
pub async fn invoke(command: LinkCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    match command {
        LinkCommand::Del(cmd) => del::invoke(cmd, output_kind).await,
        LinkCommand::Put(cmd) => put::invoke(cmd, output_kind).await,
        LinkCommand::Query(cmd) => query::invoke(cmd, output_kind).await,
    }
}
