//! Data types used when dealing with components on a wasmCloud lattice

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{ComponentId, Result};

/// A summary description of an component within a host inventory
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ComponentDescription {
    /// The unique component identifier for this component
    #[serde(default)]
    pub(crate) id: ComponentId,

    /// Image reference for this component
    #[serde(default)]
    pub(crate) image_ref: String,

    /// Name of this component, if one exists
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,

    /// The annotations that were used in the start request that produced
    /// this component instance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) annotations: Option<BTreeMap<String, String>>,

    /// The revision number for this component instance
    #[serde(default)]
    pub(crate) revision: i32,

    /// The maximum number of concurrent requests this instance can handle
    #[serde(default)]
    pub(crate) max_instances: u32,
}

#[derive(Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ComponentDescriptionBuilder {
    id: Option<ComponentId>,
    image_ref: Option<String>,
    name: Option<String>,
    annotations: Option<BTreeMap<String, String>>,
    revision: Option<i32>,
    max_instances: Option<u32>,
}

impl ComponentDescriptionBuilder {
    #[must_use]
    pub fn id(mut self, v: String) -> Self {
        self.id = Some(v);
        self
    }

    #[must_use]
    pub fn image_ref(mut self, v: String) -> Self {
        self.image_ref = Some(v);
        self
    }

    #[must_use]
    pub fn name(mut self, v: String) -> Self {
        self.name = Some(v);
        self
    }

    #[must_use]
    pub fn revision(mut self, v: i32) -> Self {
        self.revision = Some(v);
        self
    }

    #[must_use]
    pub fn max_instances(mut self, v: u32) -> Self {
        self.max_instances = Some(v);
        self
    }

    #[must_use]
    pub fn annotations(mut self, v: BTreeMap<String, String>) -> Self {
        self.annotations = Some(v);
        self
    }

    pub fn build(self) -> Result<ComponentDescription> {
        Ok(ComponentDescription {
            image_ref: self
                .image_ref
                .ok_or_else(|| "image_ref is required".to_string())?,
            id: self.id.ok_or_else(|| "id is required".to_string())?,
            name: self.name,
            revision: self.revision.unwrap_or_default(),
            max_instances: self.max_instances.unwrap_or_default(),
            annotations: self.annotations,
        })
    }
}

impl ComponentDescription {
    /// Get the ID of the component
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the image reference of the component
    pub fn image_ref(&self) -> &str {
        &self.image_ref
    }

    /// Get the name of the component
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Get the annotations of the component
    pub fn annotations(&self) -> Option<&BTreeMap<String, String>> {
        self.annotations.as_ref()
    }

    /// Get the revision of the component
    pub fn revision(&self) -> i32 {
        self.revision
    }

    /// Get the revision of the component
    pub fn max_instances(&self) -> u32 {
        self.max_instances
    }

    #[must_use]
    pub fn builder() -> ComponentDescriptionBuilder {
        ComponentDescriptionBuilder::default()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ComponentInstance {
    /// The annotations that were used in the start request that produced
    /// this component instance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) annotations: Option<BTreeMap<String, String>>,

    /// Image reference for this component
    #[serde(default)]
    pub(crate) image_ref: String,

    /// This instance's unique ID (guid)
    #[serde(default)]
    pub(crate) instance_id: String,

    /// The revision number for this component instance
    #[serde(default)]
    pub(crate) revision: i32,

    /// The maximum number of concurrent requests this instance can handle
    #[serde(default)]
    pub(crate) max_instances: u32,
}

impl ComponentInstance {
    /// Get the image reference of the component instance
    pub fn image_ref(&self) -> &str {
        &self.image_ref
    }

    /// Get the image ID of the component instance
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Get the annotations of the component
    pub fn annotations(&self) -> Option<&BTreeMap<String, String>> {
        self.annotations.as_ref()
    }

    /// Get the revision of the component
    pub fn revision(&self) -> i32 {
        self.revision
    }

    /// Get the revision of the component
    pub fn max_instances(&self) -> u32 {
        self.max_instances
    }

    #[must_use]
    pub fn builder() -> ComponentInstanceBuilder {
        ComponentInstanceBuilder::default()
    }
}

#[derive(Default, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ComponentInstanceBuilder {
    instance_id: Option<String>,
    image_ref: Option<String>,
    revision: Option<i32>,
    max_instances: Option<u32>,
    annotations: Option<BTreeMap<String, String>>,
}

impl ComponentInstanceBuilder {
    #[must_use]
    pub fn instance_id(mut self, v: String) -> Self {
        self.instance_id = Some(v);
        self
    }

    #[must_use]
    pub fn image_ref(mut self, v: String) -> Self {
        self.image_ref = Some(v);
        self
    }

    #[must_use]
    pub fn revision(mut self, v: i32) -> Self {
        self.revision = Some(v);
        self
    }

    #[must_use]
    pub fn max_instances(mut self, v: u32) -> Self {
        self.max_instances = Some(v);
        self
    }

    pub fn build(self) -> Result<ComponentInstance> {
        Ok(ComponentInstance {
            image_ref: self
                .image_ref
                .ok_or_else(|| "image_ref is required".to_string())?,
            instance_id: self
                .instance_id
                .ok_or_else(|| "id is required".to_string())?,
            revision: self.revision.unwrap_or_default(),
            max_instances: self.max_instances.unwrap_or_default(),
            annotations: self.annotations,
        })
    }
}
