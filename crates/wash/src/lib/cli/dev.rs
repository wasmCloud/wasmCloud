use anyhow::{Context as _, Result};
use console::style;
use wasmcloud_control_interface::Client;

use crate::lib::{
    build::{build_project, SignConfig},
    component::update_component,
    generate::emoji,
    id::ServerId,
    parser::{ProjectConfig, TypeConfig},
};

use super::CommonPackageArgs;

/// Perform a single execution of the dev loop for an artifact
#[allow(clippy::too_many_arguments)]
pub async fn run_dev_loop(
    project_cfg: &ProjectConfig,
    component_id: &str,
    component_ref: &str,
    host_id: &ServerId,
    ctl_client: &Client,
    sign_cfg: Option<SignConfig>,
    package_args: &CommonPackageArgs,
    skip_fetch: bool,
) -> Result<()> {
    let built_artifact_path =
        build_project(project_cfg, sign_cfg.as_ref(), package_args, skip_fetch)
            .await?
            .canonicalize()?;

    // Restart the artifact so that changes can be observed
    match project_cfg.project_type {
        TypeConfig::Provider(_) => {
            eprintln!(
                "{} {}",
                emoji::WARN,
                style("`wash build` providers are not yet supported for dev, skipping...").bold(),
            );
        }
        TypeConfig::Component(_) => {
            eprintln!(
                "{} {}",
                emoji::RECYCLE,
                style(format!(
                    "restarting component @ [{}]...",
                    built_artifact_path.display()
                ))
                .bold(),
            );

            update_component(ctl_client, host_id, component_id, component_ref)
                .await
                .context("failed to update component during dev loop")?;
        }
    }

    Ok(())
}
