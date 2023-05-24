use anyhow::Result;
use wash_lib::cli::CommandOutput;

use crate::{
    common::start_cmd::{handle_command as handle_start_command, StartCommand},
    OutputKind,
};

pub(crate) async fn handle_command(
    cli_cmd: StartCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    .await
}
