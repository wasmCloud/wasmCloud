use std::path::PathBuf;

use anyhow::{bail, Context as _, Result};
use console::style;
use wash_lib::app::AppManifest;
use wash_lib::cli::stop::stop_provider;
use wash_lib::component::{scale_component, ScaleComponentArgs};
use wasmcloud_control_interface::Client as CtlClient;

use wash_lib::build::{build_project, SignConfig};
use wash_lib::cli::CommonPackageArgs;
use wash_lib::generate::emoji;
use wash_lib::parser::ProjectConfig;

use crate::app::deploy_model_from_manifest;

use super::deps::ProjectDeps;
use super::manifest::{
    augment_existing_manifests, generate_help_text_for_manifest, generate_manifests,
};
use super::session::WashDevSession;
use super::DEFAULT_PROVIDER_STOP_TIMEOUT_MS;

/// State that is used/updated per loop of `wash dev`
pub(crate) struct RunLoopState<'a> {
    pub(crate) dev_session: &'a mut WashDevSession,
    pub(crate) nats_client: &'a async_nats::Client,
    pub(crate) secrets_transit_xkey: nkeys::XKey,
    pub(crate) secrets_subject_base: String,
    pub(crate) ctl_client: &'a CtlClient,
    pub(crate) project_cfg: &'a ProjectConfig,
    pub(crate) lattice: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) manifest_output_dir: Option<&'a PathBuf>,
    pub(crate) previous_deps: Option<ProjectDeps>,
    pub(crate) artifact_path: Option<PathBuf>,
    pub(crate) component_id: Option<String>,
    pub(crate) component_ref: Option<String>,
    pub(crate) package_args: &'a CommonPackageArgs,
    pub(crate) skip_fetch: bool,
}

/// Run one iteration of the development loop
pub(crate) async fn run(state: &mut RunLoopState<'_>) -> Result<()> {
    // Build the project (equivalent to `wash build`)
    eprintln!(
        "{} {}",
        emoji::CONSTRUCTION_BARRIER,
        style("Building project...").bold(),
    );
    // Build the project (equivalent to `wash build`)
    let built_artifact_path = match build_project(
        state.project_cfg,
        Some(&SignConfig::default()),
        state.package_args,
        state.skip_fetch,
    )
    .await
    {
        Ok(artifact_path) => artifact_path,
        Err(e) => {
            eprintln!(
                "{} {}\n{}",
                emoji::ERROR,
                style("Failed to build project:").red(),
                e
            );
            // Failing to build the project can be corrected by changing the code and shouldn't
            // stop the development loop
            return Ok(());
        }
    };
    eprintln!(
        "{} Successfully built project at [{}]",
        emoji::GREEN_CHECK,
        built_artifact_path.display()
    );

    // Update the dev loop state for reuse
    state.component_id = Some(format!(
        "{}-{}",
        state.session_id,
        state
            .project_cfg
            .common
            .name
            .to_lowercase()
            .replace(" ", "-"),
    ));
    state.component_ref = Some(format!("file://{}", built_artifact_path.display()));
    state.artifact_path = Some(built_artifact_path);

    // Generate the manifests that we need to deploy/update
    //
    // If the project configuration specified an *existing* manifest, we must merge, not generate
    let manifests = match &state.project_cfg.dev.manifests[..] {
        // If no manifest exists, we must generate one or more manifests
        [] => generate_manifests(state)
            .await
            .context("failed to generate manifests")?,
        // If manifest targets were present, use them and generate targets
        targets => augment_existing_manifests(state, targets, state.project_cfg)
            .await
            .context("failed to create manifest from existing [{}]")?,
    };

    let component_id = state
        .component_id
        .as_ref()
        .context("unexpectedly missing component_id")?;
    let component_ref = state
        .component_ref
        .as_ref()
        .context("unexpectedly missing component_ref")?;

    // Apply all manifests
    for manifest in manifests {
        // Generate all help text for this manifest
        let help_text_lines = generate_help_text_for_manifest(&manifest);

        let model_json =
            serde_json::to_string(&manifest).context("failed to convert manifest to JSON")?;

        // Put the manifest
        match wash_lib::app::put_model(
            state.nats_client,
            Some(state.lattice.to_string()),
            &model_json,
        )
        .await
        {
            Ok(_) => {}
            Err(e) if e.to_string().contains("already exists") => {}
            Err(e) => {
                bail!("failed to put model [{}]: {e}", manifest.metadata.name);
            }
        }

        // Deploy the manifest
        deploy_model_from_manifest(
            state.nats_client,
            Some(state.lattice.to_string()),
            AppManifest::ModelName(manifest.metadata.name.clone()),
            None,
        )
        .await
        .context("failed to deploy manifest")?;

        eprintln!(
            "{} {}",
            emoji::RECYCLE,
            style(format!(
                "Deployed development manifest for application [{}]",
                manifest.metadata.name,
            ))
            .bold(),
        );

        // Print all help text lines (as long as there are some)
        if !help_text_lines.is_empty() {
            eprintln!("{}", help_text_lines.join("\n"));
        }
    }

    eprintln!(
        "{} {}",
        emoji::RECYCLE,
        style(format!("Reloading component [{component_id}]...")).bold()
    );
    // Scale the component to zero, trusting that wadm will re-create it
    scale_down_component(
        state.ctl_client,
        state.project_cfg,
        &state
            .dev_session
            .host_data
            .as_ref()
            .context("missing host ID for session")?
            .0,
        component_id,
        component_ref,
    )
    .await
    .with_context(|| format!("failed to reload component [{component_id}]"))?;

    Ok(())
}

/// Scale a component to zero
pub(crate) async fn scale_down_component(
    client: &CtlClient,
    project_cfg: &ProjectConfig,
    host_id: &str,
    component_id: &str,
    component_ref: &str,
) -> Result<()> {
    // Now that backing infrastructure has changed, we should scale the component
    // as the component (if it was running before) has *not* changed.
    //
    // Scale the WADM component (which can be either a component or provider) down,
    // expecting that WADM should restore it (and trigger a reload)
    match project_cfg.project_type {
        wash_lib::parser::TypeConfig::Component(_) => {
            scale_component(ScaleComponentArgs {
                client,
                host_id,
                component_id,
                component_ref,
                max_instances: 0,
                annotations: None,
                config: vec![],
                skip_wait: false,
                timeout_ms: None,
            })
            .await
            .with_context(|| {
                format!("failed to scale down component [{component_id}] for reload")
            })?;
        }
        wash_lib::parser::TypeConfig::Provider(_) => {
            if let Err(e) = stop_provider(
                client,
                Some(host_id),
                component_id,
                false,
                DEFAULT_PROVIDER_STOP_TIMEOUT_MS,
            )
            .await
            {
                eprintln!(
                    "{} Failed to stop provider component [{component_id}] during wash dev: {e}",
                    emoji::WARN,
                );
            }
        }
    }

    Ok(())
}
