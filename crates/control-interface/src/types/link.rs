//! Data types and structure used when managing links on a wasmCloud lattice

use serde::{Deserialize, Serialize};

/// A link definition between a source and target component (component or provider) on a given
/// interface.
///
/// An [`Link`] connects one component's import to another
/// component's export, specifying the configuration each component needs in order to execute
/// the request, and represents an operator's intent to allow the source to invoke the target.
///
/// This link definition is *distinct* from the one in `wasmcloud_core`, in that it is
/// represents a link at the point in time *before* it's configuration is fully resolved
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize, Hash)]
#[non_exhaustive]
pub struct Link {
    /// Source identifier for the link
    pub(crate) source_id: String,
    /// Target for the link, which can be a unique identifier or (future) a routing group
    pub(crate) target: String,
    /// Name of the link. Not providing this is equivalent to specifying "default"
    #[serde(default = "default_link_name")]
    pub(crate) name: String,
    /// WIT namespace of the link operation, e.g. `wasi` in `wasi:keyvalue/readwrite.get`
    pub(crate) wit_namespace: String,
    /// WIT package of the link operation, e.g. `keyvalue` in `wasi:keyvalue/readwrite.get`
    pub(crate) wit_package: String,
    /// WIT Interfaces to be used for the link, e.g. `readwrite`, `atomic`, etc.
    pub(crate) interfaces: Vec<String>,
    /// List of named configurations to provide to the source upon request
    #[serde(default)]
    pub(crate) source_config: Vec<String>,
    /// List of named configurations to provide to the target upon request
    #[serde(default)]
    pub(crate) target_config: Vec<String>,
}

impl Link {
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
        &self.wit_namespace
    }

    #[must_use]
    pub fn wit_package(&self) -> &str {
        &self.wit_package
    }

    #[must_use]
    pub fn interfaces(&self) -> &Vec<String> {
        &self.interfaces
    }

    #[must_use]
    pub fn source_config(&self) -> &Vec<String> {
        &self.source_config
    }

    #[must_use]
    pub fn target_config(&self) -> &Vec<String> {
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

    pub fn build(self) -> crate::Result<Link> {
        Ok(Link {
            source_id: self
                .source_id
                .ok_or_else(|| "source id is required for creating links".to_string())?,
            target: self
                .target
                .ok_or_else(|| "target is required for creating links".to_string())?,
            name: self
                .name
                .ok_or_else(|| "name is required for creating links".to_string())?,
            wit_namespace: self
                .wit_namespace
                .ok_or_else(|| "WIT namespace is required for creating links".to_string())?,
            wit_package: self
                .wit_package
                .ok_or_else(|| "WIT package is required for creating links".to_string())?,
            interfaces: self.interfaces.unwrap_or_default(),
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
