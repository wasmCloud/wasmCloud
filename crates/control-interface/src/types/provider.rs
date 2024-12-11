//! Data types used when managing providers on a wasmCloud lattice

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{ComponentId, Result};

/// A summary description of a capability provider within a host inventory
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ProviderDescription {
    /// Provider's unique identifier
    #[serde(default)]
    pub(crate) id: ComponentId,
    /// Image reference for this provider, if applicable
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) image_ref: Option<String>,
    /// Name of the provider, if one exists
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    /// The revision of the provider
    #[serde(default)]
    pub(crate) revision: i32,
    /// The annotations that were used in the start request that produced
    /// this provider instance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) annotations: Option<BTreeMap<String, String>>,
}

impl ProviderDescription {
    /// Get the ID of the provider
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the image reference of the provider
    pub fn image_ref(&self) -> Option<&str> {
        self.image_ref.as_deref()
    }

    /// Get the name of the provider
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Get the revision of the provider
    pub fn revision(&self) -> i32 {
        self.revision
    }

    /// Get the annotations of the provider
    pub fn annotations(&self) -> Option<&BTreeMap<String, String>> {
        self.annotations.as_ref()
    }

    #[must_use]
    pub fn builder() -> ProviderDescriptionBuilder {
        ProviderDescriptionBuilder::default()
    }
}

/// Builds [`ProviderDescription`]s
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct ProviderDescriptionBuilder {
    id: Option<ComponentId>,
    image_ref: Option<String>,
    name: Option<String>,
    revision: Option<i32>,
    annotations: Option<BTreeMap<String, String>>,
}

impl ProviderDescriptionBuilder {
    /// Provider's unique identifier
    #[must_use]
    pub fn id(mut self, v: &str) -> Self {
        self.id = Some(v.into());
        self
    }

    /// Image reference for this provider, if applicable
    #[must_use]
    pub fn image_ref(mut self, v: &str) -> Self {
        self.image_ref = Some(v.into());
        self
    }

    /// Name of the provider, if one exists
    #[must_use]
    pub fn name(mut self, v: &str) -> Self {
        self.name = Some(v.into());
        self
    }

    /// The revision of the provider
    #[must_use]
    pub fn revision(mut self, v: i32) -> Self {
        self.revision = Some(v);
        self
    }

    /// The annotations that were used in the start request that produced
    /// this provider instance
    #[must_use]
    pub fn annotations(mut self, v: impl Into<BTreeMap<String, String>>) -> Self {
        self.annotations = Some(v.into());
        self
    }

    /// Build a [`ProviderDescription`]
    pub fn build(self) -> Result<ProviderDescription> {
        Ok(ProviderDescription {
            id: self.id.ok_or_else(|| "id is required".to_string())?,
            image_ref: self.image_ref,
            name: self.name,
            revision: self.revision.unwrap_or_default(),
            annotations: self.annotations,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::ProviderDescription;

    #[test]
    fn provider_description_builder() {
        assert_eq!(
            ProviderDescription {
                id: "id".into(),
                image_ref: Some("ref".into()),
                name: Some("name".into()),
                annotations: Some(BTreeMap::from([("a".into(), "b".into())])),
                revision: 0,
            },
            ProviderDescription::builder()
                .id("id")
                .image_ref("ref")
                .name("name")
                .annotations(BTreeMap::from([("a".into(), "b".into())]))
                .revision(0)
                .build()
                .unwrap()
        )
    }
}
