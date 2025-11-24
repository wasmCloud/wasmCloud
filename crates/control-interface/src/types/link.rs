//! Data types and structure used when managing links on a wasmCloud lattice

use crate::bindings::wrpc::extension::types::WitMetadata;
use serde::{Deserialize, Serialize};

pub type ComponentId = String;

/// A link definition representing how this component connects to another component through WIT interfaces.
///
/// This represents an operator's intent to allow interface communication between components,
/// with all necessary configuration resolved at the component level for the exporter component.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct InterfaceLink {
    /// The source component that exports the interfaces
    pub(crate) source_id: ComponentId,
    /// The target component that imports the interfaces
    pub(crate) target: ComponentId,
    /// Name of the link. Not providing this is equivalent to specifying "default"
    #[serde(default = "default_link_name")]
    pub(crate) name: String,
    /// Metadata about the WIT interfaces being linked across
    pub(crate) wit_metadata: WitMetadata,
    /// Interface configuration applied to either the source or target component, depending on the direction.
    #[serde(default)]
    pub(crate) source_config: Vec<String>,
    /// List of named configurations to provide to the target upon request
    #[serde(default)]
    pub(crate) target_config: Vec<String>,
}

impl InterfaceLink {
    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn wit_namespace(&self) -> &str {
        &self.wit_metadata.namespace
    }

    #[must_use]
    pub fn wit_package(&self) -> &str {
        &self.wit_metadata.package
    }

    #[must_use]
    pub fn interfaces(&self) -> &Vec<String> {
        &self.wit_metadata.interfaces
    }

    #[must_use]
    pub fn source_config(&self) -> &[String] {
        &self.source_config
    }

    #[must_use]
    pub fn target_config(&self) -> &[String] {
        &self.target_config
    }

    #[must_use]
    pub fn builder() -> LinkBuilder {
        LinkBuilder::default()
    }
}

/// Builder that produces [`Link`]s
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct LinkBuilder {
    source_id: Option<String>,
    target: Option<String>,
    name: Option<String>,
    wit_namespace: Option<String>,
    wit_package: Option<String>,
    interfaces: Option<Vec<String>>,
    source_config: Option<Vec<String>>,
    target_config: Option<Vec<String>>,
}

impl LinkBuilder {
    #[must_use]
    pub fn source_id(mut self, v: &str) -> Self {
        self.source_id = Some(v.into());
        self
    }

    #[must_use]
    pub fn target(mut self, v: &str) -> Self {
        self.target = Some(v.into());
        self
    }

    #[must_use]
    pub fn name(mut self, v: &str) -> Self {
        self.name = Some(v.into());
        self
    }

    #[must_use]
    pub fn wit_namespace(mut self, v: &str) -> Self {
        self.wit_namespace = Some(v.into());
        self
    }

    #[must_use]
    pub fn wit_package(mut self, v: &str) -> Self {
        self.wit_package = Some(v.into());
        self
    }

    #[must_use]
    pub fn interfaces(mut self, v: Vec<String>) -> Self {
        self.interfaces = Some(v);
        self
    }

    #[must_use]
    pub fn source_config(mut self, v: Vec<String>) -> Self {
        self.source_config = Some(v);
        self
    }

    #[must_use]
    pub fn target_config(mut self, v: Vec<String>) -> Self {
        self.target_config = Some(v);
        self
    }

    pub fn build(self) -> crate::Result<InterfaceLink> {
        Ok(InterfaceLink {
            source_id: self
                .source_id
                .ok_or_else(|| "source id is required for creating links".to_string())?,
            target: self
                .target
                .ok_or_else(|| "target is required for creating links".to_string())?,
            name: self
                .name
                .ok_or_else(|| "name is required for creating links".to_string())?,
            wit_metadata: WitMetadata {
                namespace: self
                    .wit_namespace
                    .ok_or_else(|| "WIT namespace is required for creating links".to_string())?,
                package: self
                    .wit_package
                    .ok_or_else(|| "WIT package is required for creating links".to_string())?,
                interfaces: self.interfaces.unwrap_or_default(),
            },
            source_config: self.source_config.unwrap_or_default(),
            target_config: self.target_config.unwrap_or_default(),
        })
    }
}

/// Helper function to provide a default link name
pub(crate) fn default_link_name() -> String {
    "default".to_string()
}

#[cfg(test)]
mod tests {

    use super::Link;

    #[test]
    fn link_builder() {
        assert_eq!(
            Link {
                source_id: "source_id".into(),
                target: "target".into(),
                name: "name".into(),
                wit_namespace: "wit_namespace".into(),
                wit_package: "wit_package".into(),
                interfaces: vec!["i".into()],
                source_config: vec!["sc".into()],
                target_config: vec!["tc".into()]
            },
            Link::builder()
                .source_id("source_id")
                .target("target")
                .name("name")
                .wit_namespace("wit_namespace")
                .wit_package("wit_package")
                .interfaces(vec!["i".into()])
                .source_config(vec!["sc".into()])
                .target_config(vec!["tc".into()])
                .build()
                .unwrap()
        );
    }
}
