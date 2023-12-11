use anyhow::Result;
use console::style;
use wasmcloud_control_interface::Client;

use crate::{
    actor::{start_actor, update_actor, StartActorArgs},
    build::{build_project, SignConfig},
    generate::emoji,
    id::{ModuleId, ServerId},
    parser::{ProjectConfig, TypeConfig},
};

/// Perform a single execution of the dev loop for an artifact
pub async fn run_dev_loop(
    project_cfg: &ProjectConfig,
    actor_id: ModuleId,
    actor_ref: &str,
    host_id: ServerId,
    ctl_client: &Client,
    sign_cfg: Option<SignConfig>,
) -> Result<()> {
    let built_artifact_path = build_project(project_cfg, sign_cfg)?.canonicalize()?;

    // Restart the artifact so that changes can be observed
    match project_cfg.project_type {
        TypeConfig::Interface(_) | TypeConfig::Provider(_) => {
            eprintln!(
                "{} {}",
                emoji::WARN,
                style("`wash build` interfaces and providers are not yet supported, skipping...")
                    .bold(),
            );
        }
        TypeConfig::Actor(_) => {
            eprintln!(
                "{} {}",
                emoji::RECYCLE,
                style(format!(
                    "restarting actor @ [{}]...",
                    built_artifact_path.display()
                ))
                .bold(),
            );

            update_actor(ctl_client, &host_id, &actor_id, actor_ref).await?;
        }
    }

    Ok(())
}
