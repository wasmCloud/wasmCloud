use clap::Args;
use tracing::instrument;

use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    inspect::{decode_component, get_component_wit},
};
use anyhow::Context;
use std::path::Path;

#[derive(Args, Debug, Clone)]
pub struct InspectCommand {
    /// Inspect a component at a given path.
    #[arg(value_name = "COMPONENT_PATH")]
    pub component_reference: String,
}

impl CliCommand for InspectCommand {
    #[instrument(level = "debug", skip_all, name = "inspect")]
    async fn handle(&self, _ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        // Handle the optional component reference - default to current directory if not provided
        let component_reference = &self.component_reference;

        let path = Path::new(&component_reference);

        let bytes = if path.exists() {
            if path.is_file() {
                tokio::fs::read(&component_reference)
                    .await
                    .context("failed to read component file")?
            } else if path.is_dir() {
                anyhow::bail!(
                    "Directory '{}' specified. Please provide a file path.",
                    component_reference
                );
            } else {
                anyhow::bail!(
                    "Path '{}' exists but is neither a file nor directory",
                    component_reference
                );
            }
        } else {
            anyhow::bail!("Path '{}' does not exist locally", component_reference);
        };

        let component = decode_component(bytes.as_slice())
            .await
            .context("failed to decode component")?;

        // Print the component WIT
        let wit = get_component_wit(component)
            .await
            .context("failed to print component WIT")?;

        Ok(CommandOutput::ok(
            wit.to_owned(),
            Some(serde_json::json!({
                "message": "Component inspected successfully.",
                "success": true,
                "wit": wit,
            })),
        ))
    }
}
