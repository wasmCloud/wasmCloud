use anyhow::Result;

use wash_lib::cli::{
    update::{handle_update_actor, UpdateCommand},
    CommandOutput, OutputKind,
};

use crate::appearance::spinner::Spinner;

pub async fn handle_command(
    command: UpdateCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out = match command {
        UpdateCommand::Actor(cmd) => {
            sp.update_spinner_message(format!(
                " Updating Actor {} to {} ... ",
                cmd.actor_id, cmd.new_actor_ref
            ));

            handle_update_actor(cmd).await?
        }
    };

    sp.finish_and_clear();

    Ok(out)
}
