use anyhow::Result;

use wash_lib::cli::{
    logs::{handle_get_logging_config, handle_set_logging_config, LogsCommand},
    CommandOutput, OutputKind,
};

use crate::appearance::spinner::Spinner;

pub async fn handle_command(
    command: LogsCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out: CommandOutput = match command {
        LogsCommand::Get(cmd) => handle_get_logging_config(cmd).await?,
        LogsCommand::Set(cmd) => handle_set_logging_config(cmd).await?,
    };

    sp.finish_and_clear();

    Ok(out)
}
