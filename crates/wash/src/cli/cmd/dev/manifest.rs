use anyhow::Result;
use console::style;
use wadm_types::{
    CapabilityProperties, Component, ComponentProperties, Manifest, Properties,
    SpreadScalerProperty, TraitProperty,
};

use crate::lib::{generate::emoji, parser::ProjectConfig};

/// Generate the a configuration name for a dependency, given it's namespace and package
pub fn config_name(ns: &str, pkg: &str) -> String {
    format!("{ns}-{pkg}-config")
}

/// Find the first config value for provider link trait which has a certain name
///
/// This is normally used to extract a specific config property that we expect
/// to be in a the config properties for a link attached to the component
pub fn find_provider_source_trait_config_value<'a>(
    component: &'a Component,
    config_name_predicate: impl Fn(&str) -> bool,
    property_key: &'a str,
) -> Option<&'a str> {
    // Retrieve link traits
    if let Some(link_traits) = component
        .traits
        .as_ref()
        .map(|ts| ts.iter().filter(|t| t.is_link()))
    {
        for link_trait in link_traits {
            if let TraitProperty::Link(l) = &link_trait.properties {
                if let Some(def) = &l.source {
                    for cfg in &def.config {
                        if let (name, Some(Some(value))) = (
                            &cfg.name,
                            cfg.properties.as_ref().map(|p| p.get(property_key)),
                        ) {
                            if config_name_predicate(name.as_str()) {
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
pub fn generate_help_text_for_manifest(manifest: &Manifest) -> Vec<String> {
    let mut lines = Vec::new();
    for component in &manifest.spec.components {
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
                    // Sometimes we might want a *specific* config name (ex. output of config_name(...)),
                    // but here we allow any to support cases like custom manifests or overrides
                    |_name| true,
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
                    // Sometimes we might want a *specific* config name (ex. output of config_name(...)),
                    // but here we allow any to support cases like custom manifests or overrides
                    |_name| true,
                    "subscriptions",
                ) {
                    lines.push(format!(
                        "{} {}",
                        emoji::SPARKLE,
                        style(format!(
                            "Messaging NATS: Listening on the following subscriptions [{}]",
                            subscriptions.split(',').collect::<Vec<&str>>().join(", "),
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
pub fn generate_component_from_project_cfg(
    cfg: &ProjectConfig,
    component_id: &str,
    image_ref: &str,
) -> Result<Component> {
    Ok(Component {
        name: component_id.into(),
        properties: match &cfg.project_type {
            crate::lib::parser::TypeConfig::Component(_c) => Properties::Component {
                properties: ComponentProperties {
                    image: Some(image_ref.into()),
                    application: None,
                    id: Some(component_id.into()),
                    config: Vec::with_capacity(0),
                    secrets: Vec::with_capacity(0),
                },
            },
            crate::lib::parser::TypeConfig::Provider(_p) => Properties::Capability {
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
            crate::lib::parser::TypeConfig::Component(_c) => Some(vec![wadm_types::Trait {
                trait_type: "spreadscaler".into(),
                properties: TraitProperty::SpreadScaler(SpreadScalerProperty {
                    instances: 100,
                    spread: Vec::new(),
                }),
            }]),
            crate::lib::parser::TypeConfig::Provider(_p) => Some(vec![wadm_types::Trait {
                trait_type: "spreadscaler".into(),
                properties: TraitProperty::SpreadScaler(SpreadScalerProperty {
                    instances: 1,
                    spread: Vec::new(),
                }),
            }]),
        },
    })
}
