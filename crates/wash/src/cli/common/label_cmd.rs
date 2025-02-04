use anyhow::Result;

use crate::lib::cli::{
    label::{handle_label_host, LabelHostCommand},
    CommandOutput, OutputKind,
};

use crate::appearance::spinner::Spinner;

pub async fn handle_command(
    cmd: LabelHostCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out = handle_label_host(cmd).await?;
    sp.finish_and_clear();

    Ok(out)
}
