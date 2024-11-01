use std::collections::{BTreeSet, HashMap};

use anyhow::{ensure, Context as _, Result};
use console::style;

use secrets_nats_kv::PutSecretRequest;
use wadm_types::{
    CapabilityProperties, Component, ComponentProperties, ConfigProperty, Manifest, Properties,
    SecretProperty, SecretSourceProperty, SpreadScalerProperty, TraitProperty,
};
use wash_lib::generate::emoji;
use wash_lib::parser::{
    DevConfigSpec, DevManifestComponentTarget, DevSecretSpec, ProjectConfig, TypeConfig,
};

use super::deps::{DependencySpec, ProjectDependencyKey, ProjectDeps};
use super::devloop::{scale_down_component, RunLoopState};
use super::wit::{discover_dependencies_from_wit, parse_component_wit, parse_project_wit};

/// Generate the a configuration name for a dependency, given it's namespace and package
pub(crate) fn config_name(ns: &str, pkg: &str) -> String {
    format!("{}-{}-config", ns, pkg)
}

/// Find the first config value for provider  trait configuration configuration which has a certain name
pub(crate) fn find_provider_source_trait_config_value<'a>(
    component: &'a Component,
    config_name: &'a str,
    property_key: &'a str,
) -> Option<&'a str> {
    // Retrieve link traits
    if let Some(link_traits) = component
        .traits
        .as_ref()
        .map(|ts| ts.iter().filter(|t| t.is_link()))
    {
        // Find the first link config that is named "default" and has "address"
        for link_trait in link_traits {
            if let TraitProperty::Link(l) = &link_trait.properties {
                if let Some(def) = &l.source {
                    for cfg in &def.config {
                        if let (name, Some(Some(value))) = (
                            &cfg.name,
                            cfg.properties.as_ref().map(|p| p.get(property_key)),
                        ) {
                            if name == config_name {
                                return Some(value);
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

/// Generate help text for manifest with components that we recognize
pub(crate) fn generate_help_text_for_manifest(manifest: &Manifest) -> Vec<String> {
    let mut lines = Vec::new();
    for component in manifest.spec.components.iter() {
        match &component.properties {
            // Add help text for HTTP server
            Properties::Capability {
                properties:
                    CapabilityProperties {
                        image: Some(image), ..
                    },
            } if image.starts_with("ghcr.io/wasmcloud/http-server") => {
                if let Some(address) = find_provider_source_trait_config_value(
                    component,
                    &config_name("wasi", "http"),
                    "address",
                ) {
                    lines.push(format!(
                        "{} {}",
                        emoji::SPARKLE,
                        style(format!(
                            "HTTP Server: Access your application at {}",
                            if address.starts_with("http") {
                                address.into()
                            } else {
                                format!("http://{address}")
                            }
                        ))
                        .bold()
                    ));
                }
            }
            // Add help text for Messaging server
            Properties::Capability {
                properties:
                    CapabilityProperties {
                        image: Some(image), ..
                    },
            } if image.starts_with("ghcr.io/wasmcloud/messaging-nats") => {
                if let Some(subscriptions) = find_provider_source_trait_config_value(
                    component,
                    &config_name("wasmcloud", "messaging"),
                    "subscriptions",
                ) {
                    lines.push(format!(
                        "{} {}",
                        emoji::SPARKLE,
                        style(format!(
                            "Messaging NATS: Listening on the following subscriptions [{}]",
                            subscriptions.split(",").collect::<Vec<&str>>().join(", "),
                        ))
                        .bold()
                    ));
                }
            }
            _ => {}
        }
    }

    lines
}

/// Generate a WADM component from a project configuration
pub(crate) fn generate_component_from_project_cfg(
    cfg: &ProjectConfig,
    component_id: &str,
    image_ref: &str,
) -> Result<Component> {
    Ok(Component {
        name: component_id.into(),
        properties: match &cfg.project_type {
            wash_lib::parser::TypeConfig::Component(_c) => Properties::Component {
                properties: ComponentProperties {
                    image: Some(image_ref.into()),
                    application: None,
                    id: Some(component_id.into()),
                    config: Vec::with_capacity(0),
                    secrets: Vec::with_capacity(0),
                },
            },
            wash_lib::parser::TypeConfig::Provider(_p) => Properties::Capability {
                properties: CapabilityProperties {
                    image: Some(image_ref.into()),
                    application: None,
                    id: Some(component_id.into()),
                    config: Vec::with_capacity(0),
                    secrets: Vec::with_capacity(0),
                },
            },
        },
        traits: match &cfg.project_type {
            wash_lib::parser::TypeConfig::Component(_c) => Some(vec![wadm_types::Trait {
                trait_type: "spreadscaler".into(),
                properties: TraitProperty::SpreadScaler(SpreadScalerProperty {
                    instances: 100,
                    spread: Vec::new(),
                }),
            }]),
            wash_lib::parser::TypeConfig::Provider(_p) => Some(vec![wadm_types::Trait {
                trait_type: "spreadscaler".into(),
                properties: TraitProperty::SpreadScaler(SpreadScalerProperty {
                    instances: 1,
                    spread: Vec::new(),
                }),
            }]),
        },
    })
}

/// Update config properties (normally part of a [`Component`] in a [`Manifest`]) with a given config spec
async fn update_secret_properties_by_spec(
    state: &RunLoopState<'_>,
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
                })
            }
        }
        DevSecretSpec::Values { name, values } => {
            // Go through all provided values and build the secret a secret
            for (k, v) in values {
                // If a secret was provided from ENV, then create it
                if let Some((_, env_var)) = v.split_once("$ENV:") {
                    if let Ok(v) = std::env::var(env_var) {
                        let psr = PutSecretRequest {
                            key: k.into(),
                            string_secret: Some(v.to_string()),
                            ..Default::default()
                        };
                        secrets_nats_kv::client::put_secret(
                            state.nats_client,
                            &state.secrets_subject_base,
                            &state.secrets_transit_xkey,
                            psr,
                        )
                        .await
                        .with_context(|| format!("failed to input secret property [{name}"))?;
                    }
                }

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

/// Load existing manifests specified
///
/// # Arguments
///
/// * `manifest_paths` - paths to manifest
/// * `project_config` - Project configuration
/// * `component_id` - ID of the component under development
/// * `component_ref` - Image ref of the component under development
///
pub(crate) async fn augment_existing_manifests(
    state: &RunLoopState<'_>,
    manifest_paths: &[DevManifestComponentTarget],
    project_config: &ProjectConfig,
) -> Result<Vec<Manifest>> {
    let generated_component_id = state
        .component_id
        .as_ref()
        .context("missing component_id")?;
    let generated_component_ref = state
        .component_ref
        .as_ref()
        .context("missing component id")?;
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
                            "failed to update secret proeprties for component [{}]",
                            component.name
                        )
                    })?;
            }

            // Apply secret specs
            for spec in &project_config.dev.secrets {
                update_secret_properties_by_spec(state, secrets, spec)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to update secret proeprties for component [{}]",
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
                })
            }
        }
        DevConfigSpec::Values { values } => {
            // Add values explicitly to the bottom of the list, overriding the others
            configs.push(ConfigProperty {
                name: "dev-overrides".into(),
                properties: Some(HashMap::from_iter(values.clone())),
            })
        }
    }
    Ok(())
}

/// Generate manifests that should be deployed, based on the current run loop state
pub(crate) async fn generate_manifests(
    RunLoopState {
        dev_session,
        ctl_client,
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
    eprintln!(
        "{} Detected component dependencies: {:?}",
        emoji::INFO_SQUARE,
        wit_implied_deps
            .iter()
            .map(DependencySpec::name)
            .collect::<BTreeSet<String>>()
    );
    let pkey = ProjectDependencyKey::from_project(
        &project_cfg.common.name,
        &project_cfg.common.project_dir,
    )
    .context("failed to build key for project")?;

    let mut current_project_deps = match previous_deps {
        Some(deps) => deps.clone(),
        None => ProjectDeps::from_known_deps(pkey.clone(), wit_implied_deps)
            .context("failed to build project dependencies")?,
    };

    // Pull and merge in overrides from project-level wasmcloud.toml
    let project_override_deps = ProjectDeps::from_project_config_overrides(pkey, project_cfg)
        .with_context(|| {
            format!(
                "failed to discover project dependencies from project [{}]",
                project_cfg.common.project_dir.display(),
            )
        })?;
    current_project_deps
        .merge_override(project_override_deps)
        .context("failed to merge & override project-specified deps")?;

    // After we've merged, we can update the session ID to belong to this session
    current_project_deps.session_id = Some(session_id.to_string());

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
    if project_deps_unchanged {
        eprintln!(
            "{} {}",
            emoji::RECYCLE,
            style(format!(
                "(Fast-)Reloading component [{component_id}] (no dependencies have changed)..."
            ))
            .bold()
        );
        // Scale the component to zero, trusting that wadm will re-create it
        scale_down_component(
            ctl_client,
            project_cfg,
            &dev_session
                .host_data
                .as_ref()
                .context("missing host ID for session")?
                .0,
            component_id,
            component_ref,
        )
        .await
        .with_context(|| format!("failed to reload component [{component_id}]"))?;

        // Return with no generated manifests
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
        for manifest in manifests.iter() {
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
            })?
        }
    }

    // Update deps, since they must be different
    *previous_deps = Some(current_project_deps);

    Ok(manifests)
}
