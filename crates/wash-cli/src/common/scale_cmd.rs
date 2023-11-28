use anyhow::Result;

use wash_lib::cli::{
    scale::{handle_scale_actor, ScaleCommand},
    CommandOutput, OutputKind,
};

use crate::appearance::spinner::Spinner;

pub async fn handle_command(
    command: ScaleCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let sp: Spinner = Spinner::new(&output_kind)?;
    let out = match command {
        ScaleCommand::Actor(cmd) => {
            sp.update_spinner_message(format!(
                " Scaling Actor {} to {} max concurrent instances ... ",
                cmd.actor_ref, cmd.max_instances
            ));
            handle_scale_actor(cmd.clone()).await?
        }
    };

    sp.finish_and_clear();

    Ok(out)
}
