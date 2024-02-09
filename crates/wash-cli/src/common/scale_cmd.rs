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
            let scale_msg = if cmd.max_instances == u32::MAX {
                "unbounded concurrency".to_string()
            } else {
                format!("{} max concurrent instances", cmd.max_instances)
            };
            sp.update_spinner_message(format!(
                " Sending request to scale actor {} to {scale_msg} ... ",
                cmd.actor_ref
            ));
            handle_scale_actor(cmd.clone()).await?
        }
    };

    sp.finish_and_clear();

    Ok(out)
}
