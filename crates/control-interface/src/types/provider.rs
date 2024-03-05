//! Data types used when managing providers on a wasmCloud lattice

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ComponentId;

/// A summary description of a capability provider within a host inventory
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderDescription {
    /// The annotations that were used in the start request that produced
    /// this provider instance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<HashMap<String, String>>,
    /// Provider's unique identifier
    #[serde(default)]
    pub id: ComponentId,
    /// Image reference for this provider, if applicable
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<String>,
    /// Name of the provider, if one exists
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The revision of the provider
    #[serde(default)]
    pub revision: i32,
}
