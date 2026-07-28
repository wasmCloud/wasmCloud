use clap::Args;
use tracing::instrument;
use wash_runtime::component_source::ComponentSource;

use crate::{
    cli::{CliCommand, CliContext, CommandOutput, oci::RegistryArgs},
    inspect::{decode_component, get_component_wit},
};
use anyhow::{Context, ensure};
use std::path::Path;

#[derive(Args, Debug, Clone)]
pub struct InspectCommand {
    /// Inspect a component, given either a local path or an OCI reference.
    #[arg(value_name = "COMPONENT_REFERENCE")]
    pub component_reference: String,
    /// Registry settings, used only when the reference is an OCI image.
    #[command(flatten)]
    pub registry: RegistryArgs,
}

impl CliCommand for InspectCommand {
    #[instrument(level = "debug", skip_all, name = "inspect")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let component_reference = &self.component_reference;

        // A directory is the one input the shared source resolution cannot make
        // sense of: it exists, so it is classified as a file, and reading it
        // fails with an OS error that does not say what to do instead.
        let path = Path::new(component_reference);
        ensure!(
            !path.is_dir(),
            "Directory '{component_reference}' specified. Please provide a file path or OCI reference."
        );

        let loaded = ComponentSource::from_reference(component_reference)
            .load(self.registry.oci_config(ctx))
            .await?;

        let component = decode_component(loaded.bytes.as_ref())
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
