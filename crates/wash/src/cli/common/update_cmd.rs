use anyhow::Result;

use crate::lib::cli::{
    update::{handle_update_component, UpdateCommand},
    CommandOutput, OutputKind,
};

use crate::appearance::spinner::Spinner;

pub async fn handle_command(
    command: UpdateCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out = match command {
        UpdateCommand::Component(cmd) => {
            sp.update_spinner_message(format!(
                " Updating Component {} to {} ... ",
                cmd.component_id, cmd.new_component_ref
            ));

            handle_update_component(cmd).await?
        }
    };

    sp.finish_and_clear();

    Ok(out)
}
