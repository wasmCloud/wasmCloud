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
            let max = cmd
                .max_concurrent
                .map(|max| max.to_string())
                .unwrap_or_else(|| "unbounded".to_string());
            sp.update_spinner_message(format!(
                " Scaling Actor {} to {} max concurrent instances ... ",
                cmd.actor_ref, max
            ));
            handle_scale_actor(cmd.clone()).await?
        }
    };

    sp.finish_and_clear();

    Ok(out)
}
