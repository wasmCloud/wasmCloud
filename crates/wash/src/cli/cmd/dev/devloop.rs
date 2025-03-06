use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use anyhow::{bail, ensure, Context as _, Result};
use console::style;
use tracing::{debug, warn};
use crate::lib::app::AppManifest;
use crate::lib::cli::stop::stop_provider;
use crate::lib::component::{scale_component, ScaleComponentArgs};
use wasmcloud_control_interface::Client as CtlClient;

use wadm_types::{ConfigProperty, Manifest, Properties, SecretProperty, SecretSourceProperty};
use crate::lib::build::{build_project, SignConfig};
use crate::lib::cli::{CommonPackageArgs, OutputKind};
use crate::lib::generate::emoji;
use crate::lib::parser::{
    load_config, DevConfigSpec, DevManifestComponentTarget, DevSecretSpec, ProjectConfig,
    TypeConfig,
};

use crate::app::deploy_model_from_manifest;
use crate::appearance::spinner::Spinner;

use super::deps::{DependencySpec, ProjectDependencyKey, ProjectDeps};
use super::manifest::{generate_component_from_project_cfg, generate_help_text_for_manifest};
use super::session::WashDevSession;
use super::wit::{discover_dependencies_from_wit, parse_component_wit, parse_project_wit};
use super::DEFAULT_PROVIDER_STOP_TIMEOUT_MS;

/// State that is used/updated per loop of `wash dev`
pub struct RunLoopState<'a> {
    pub(crate) dev_session: &'a mut WashDevSession,
    pub(crate) nats_client: &'a async_nats::Client,
    pub(crate) ctl_client: &'a CtlClient,
    pub(crate) project_cfg: &'a mut ProjectConfig,
    pub(crate) lattice: &'a str,
    pub(crate) session_id: &'a str,
    pub(crate) manifest_output_dir: Option<&'a PathBuf>,
    pub(crate) previous_deps: Option<ProjectDeps>,
    pub(crate) artifact_path: Option<PathBuf>,
    pub(crate) component_id: Option<String>,
    pub(crate) component_ref: Option<String>,
    pub(crate) package_args: &'a CommonPackageArgs,
    pub(crate) skip_fetch: bool,
    pub(crate) output_kind: OutputKind,
}

/// Generate manifests that should be deployed, based on the current run loop state
pub async fn generate_manifests(
    RunLoopState {
        project_cfg,
        session_id,
        ref mut previous_deps,
        artifact_path,
        component_id,
        component_ref,
        manifest_output_dir,
        ..
    }: &mut RunLoopState<'_>,
) -> Result<Vec<Manifest>> {
    let artifact_path = artifact_path.as_ref().context("missing artifact path")?;
    // After the project is built, we must ensure dependencies are set up and running
    let (resolve, world_id) = if let TypeConfig::Component(_) = project_cfg.project_type {
        let component_bytes = tokio::fs::read(&artifact_path).await.with_context(|| {
            format!(
                "failed to read component bytes from built artifact path {}",
                artifact_path.display()
            )
        })?;
        parse_component_wit(&component_bytes).context("failed to parse WIT from component")?
    } else {
        parse_project_wit(project_cfg).context("failed to parse WIT from project dir")?
    };

    // Pull implied dependencies from WIT
    let wit_implied_deps = discover_dependencies_from_wit(resolve, world_id)
        .context("failed to resolve dependent components")?;

    let pkey = ProjectDependencyKey::from_project(
        &project_cfg.common.name,
        &project_cfg.common.project_dir,
    )
    .context("failed to build key for project")?;

    let mut current_project_deps = ProjectDeps::from_known_deps(pkey.clone(), wit_implied_deps)
        .context("failed to build project dependencies")?;
    // Pull and merge in overrides from project-level wasmcloud.toml
    let project_override_deps = ProjectDeps::from_project_config_overrides(pkey, project_cfg)
        .with_context(|| {
            format!(
                "failed to discover project dependencies from config [{}]",
                project_cfg.common.project_dir.display(),
            )
        })?;
    current_project_deps
        .merge_override(project_override_deps)
        .context("failed to merge & override project-specified deps")?;
    eprintln!(
        "{} Detected component dependencies: {:?}",
        emoji::INFO_SQUARE,
        current_project_deps
            .dependencies
            .values()
            .flatten()
            .map(DependencySpec::name)
            .collect::<BTreeSet<String>>()
    );

    // After we've merged, we can update the session ID to belong to this session
    current_project_deps.session_id = Some((**session_id).to_string());

    // Generate component that represents the main Webassembly component/provider being developed
    let component_id = component_id.as_ref().context("missing component id")?;
    let component_ref = component_ref.as_ref().context("missing component ref")?;
    current_project_deps.component =
        generate_component_from_project_cfg(project_cfg, component_id, component_ref)
            .map(Some)
            .context("failed to generate app component")?;

    // If deps haven't changed, then we can simply restart the component and return
    let project_deps_unchanged = previous_deps
        .as_ref()
        .is_some_and(|deps| deps.eq(&current_project_deps));
    // Return with no generated manifests if deps haven't changed
    if project_deps_unchanged {
        return Ok(Vec::new());
    }

    // Convert the project deps into a fully-baked WADM manifests
    let manifests = current_project_deps
        .generate_wadm_manifests()
        .with_context(|| {
            format!("failed to generate a WADM manifest from (session [{session_id}])")
        })?
        .into_iter()
        .collect::<Vec<_>>();

    // Write out manifests to local files if a manifest output dir was specified
    if let Some(output_dir) = &manifest_output_dir {
        for manifest in &manifests {
            ensure!(
                tokio::fs::metadata(output_dir)
                    .await
                    .context("failed to get manifest output dir metadata")
                    .is_ok_and(|f| f.is_dir()),
                "manifest output directory [{}] must exist and be a folder",
                output_dir.display()
            );
            tokio::fs::write(
                output_dir.join(format!("{}.yaml", manifest.metadata.name)),
                serde_yaml::to_string(&manifest).context("failed to convert manifest to YAML")?,
            )
            .await
            .with_context(|| {
                format!(
                    "failed to write out manifest YAML to output dir [{}]",
                    output_dir.display(),
                )
            })?;
        }
    }

    // Update deps, since they must be different
    *previous_deps = Some(current_project_deps);

    Ok(manifests)
}

/// Load existing manifests specified
///
/// # Arguments
///
/// * `manifest_paths` - paths to manifest
/// * `project_config` - Project configuration
/// * `component_id` - ID of the component under development
/// * `component_ref` - Image ref of the component under development
///
async fn augment_existing_manifests(
    manifest_paths: &Vec<DevManifestComponentTarget>,
    project_config: &ProjectConfig,
    generated_component_id: &str,
    generated_component_ref: &str,
) -> Result<Vec<Manifest>> {
    let mut manifests = Vec::with_capacity(manifest_paths.len());
    for component_target in manifest_paths {
        // Read the manifest
        let mut manifest = serde_yaml::from_slice::<Manifest>(
            &tokio::fs::read(&component_target.path)
                .await
                .with_context(|| {
                    format!(
                        "failed to read manifest @ [{}]",
                        component_target.path.display()
                    )
                })?,
        )
        .context("failed to parse manifest YAML")?;

        // Augment the manifest with the component, if present
        for component in manifest.spec.components.as_mut_slice() {
            // If neither the component ID nor the ref match, then skip
            if !component_target.matches(component) {
                continue;
            }

            // Once we know we're on a component that matches we can extract information to modify
            let (id, image_ref, config, secrets) = match &mut component.properties {
                Properties::Component { ref mut properties } => (
                    &mut properties.id,
                    &mut properties.image,
                    &mut properties.config,
                    &mut properties.secrets,
                ),
                Properties::Capability { ref mut properties } => (
                    &mut properties.id,
                    &mut properties.image,
                    &mut properties.config,
                    &mut properties.secrets,
                ),
            };

            // Update the ID and image ref
            *id = Some(generated_component_id.into());
            *image_ref = Some(generated_component_ref.into());

            // Apply config specs
            for spec in &project_config.dev.config {
                update_config_properties_by_spec(config, spec)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to update secret properties for component [{}]",
                            component.name
                        )
                    })?;
            }

            // Apply secret specs
            for spec in &project_config.dev.secrets {
                update_secret_properties_by_spec(secrets, spec)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to update secret properties for component [{}]",
                            component.name
                        )
                    })?;
            }
        }

        manifests.push(manifest);
    }

    Ok(manifests)
}

/// Update config properties (normally part of a [`Component`] in a [`Manifest`]) with a given config spec
async fn update_config_properties_by_spec(
    configs: &mut Vec<ConfigProperty>,
    spec: &DevConfigSpec,
) -> Result<()> {
    match spec {
        DevConfigSpec::Named { name } => {
            // Add any named configs that are not present
            if !configs.iter().any(|c| c.name == *name) {
                configs.push(ConfigProperty {
                    name: name.to_string(),
                    properties: None,
                });
            }
        }
        DevConfigSpec::Values { values } => {
            // Add values explicitly to the bottom of the list, overriding the others
            configs.push(ConfigProperty {
                name: "dev-overrides".into(),
                properties: Some(HashMap::from_iter(values.clone())),
            });
        }
    }
    Ok(())
}

/// Update config properties (normally part of a [`Component`] in a [`Manifest`]) with a given config spec
async fn update_secret_properties_by_spec(
    secrets: &mut Vec<SecretProperty>,
    spec: &DevSecretSpec,
) -> Result<()> {
    match spec {
        DevSecretSpec::Existing { name, source } => {
            // Add any named secrets that are not present
            if !secrets.iter().any(|c| c.name == *name) {
                secrets.push(SecretProperty {
                    name: name.to_string(),
                    properties: source.clone(),
                });
            }
        }
        DevSecretSpec::Values { name, values } => {
            // Go through all provided values and build the secret a secret
            for (k, v) in values {
                ensure!(
                    !v.starts_with("$ENV:"),
                    "ENV-loaded secrets are not yet supported"
                );

                // Add values explicitly to the bottom of the list, overriding the others
                secrets.push(SecretProperty {
                    name: name.to_string(),
                    properties: SecretSourceProperty {
                        policy: "nats-kv".into(),
                        key: k.into(),
                        field: None,
                        version: None,
                    },
                });
            }
        }
    }
    Ok(())
}

/// Run one iteration of the development loop
pub async fn run(state: &mut RunLoopState<'_>) -> Result<()> {
    // Build the project (equivalent to `wash build`)
    let spinner = Spinner::new(&state.output_kind).context("failed to create spinner")?;
    if matches!(state.output_kind, OutputKind::Text) {
        spinner.update_spinner_message("Building project...");
    } else {
        eprintln!(
            "{} {}",
            emoji::CONSTRUCTION_BARRIER,
            style("Building project...").bold(),
        );
    }
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
    spinner.finish_and_clear();
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
            .replace(' ', "-"),
    ));
    state.component_ref = Some(format!("file://{}", built_artifact_path.display()));
    state.artifact_path = Some(built_artifact_path);
    match load_config(Some(state.dev_session.project_path.clone()), Some(true)).await {
        Ok(cfg) => *state.project_cfg = cfg,
        Err(e) => {
            warn!(err = ?e, "failed to load project configuration, using previous configuration");
        }
    }

    // Generate the manifests that we need to deploy/update
    //
    // If the project configuration specified an *existing* manifest, we must merge, not generate
    let manifests = if !state.project_cfg.dev.manifests.is_empty() {
        augment_existing_manifests(
            &state.project_cfg.dev.manifests,
            state.project_cfg,
            state
                .component_id
                .as_ref()
                .context("missing component_id")?,
            state
                .component_ref
                .as_ref()
                .context("missing component id")?,
        )
        .await
        .context("failed to create manifest from existing")?
    } else {
        // If no manifest exists, we must generate one or more manifests
        generate_manifests(state)
            .await
            .context("failed to generate manifests")?
    };

    let component_id = state
        .component_id
        .as_ref()
        .context("unexpectedly missing component_id")?;
    let component_ref = state
        .component_ref
        .as_ref()
        .context("unexpectedly missing component_ref")?;

    // If manifests are empty, let the user know we're not deploying anything, just reloading
    // the same component
    if manifests.is_empty() {
        eprintln!(
            "{} {}",
            emoji::RECYCLE,
            style(format!(
                "(Fast-)Reloading component [{component_id}] (no dependencies have changed)..."
            ))
            .bold()
        );
    } else {
        eprintln!(
            "{} {}",
            emoji::RECYCLE,
            style(format!("Reloading component [{component_id}]...")).bold()
        );
    }

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

    // Apply all manifests
    for manifest in manifests {
        // Generate all help text for this manifest
        let help_text_lines = generate_help_text_for_manifest(&manifest);

        let model_json =
            serde_json::to_string(&manifest).context("failed to convert manifest to JSON")?;

        // Put the manifest
        match crate::lib::app::put_model(
            state.nats_client,
            Some(state.lattice.to_string()),
            &model_json,
        )
        .await
        {
            Ok(_) => {
                debug!(
                    name = manifest.metadata.name.as_str(),
                    "successfully put application",
                );
            }
            Err(e) if e.to_string().contains("already exists") => {
                warn!(
                    name = manifest.metadata.name.as_str(),
                    "application already exists, skipping put",
                );
            }
            Err(e) => {
                bail!(
                    "failed to put application [{}]: {e}",
                    manifest.metadata.name
                );
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
                "Deployed updated manifest for application [{}]",
                manifest.metadata.name,
            ))
            .bold(),
        );

        // Print all help text lines (as long as there are some)
        if !help_text_lines.is_empty() {
            eprintln!("{}", help_text_lines.join("\n"));
        }
    }

    Ok(())
}

/// Scale a component to zero
async fn scale_down_component(
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
        crate::lib::parser::TypeConfig::Component(_) => {
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
        crate::lib::parser::TypeConfig::Provider(_) => {
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
