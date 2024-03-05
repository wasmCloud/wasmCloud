//! Data types used when dealing with actors on a wasmCloud lattice

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ComponentId;

/// A summary description of an actor within a host inventory
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorDescription {
    /// The unique component identifier for this actor
    #[serde(default)]
    pub id: ComponentId,
    /// Image reference for this actor
    #[serde(default)]
    pub image_ref: String,
    /// Name of this actor, if one exists
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The annotations that were used in the start request that produced
    /// this actor instance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<HashMap<String, String>>,
    /// The revision number for this actor instance
    #[serde(default)]
    pub revision: i32,
    /// The maximum number of concurrent requests this instance can handle
    #[serde(default)]
    pub max_instances: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorInstance {
    /// The annotations that were used in the start request that produced
    /// this actor instance
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<HashMap<String, String>>,
    /// Image reference for this actor
    #[serde(default)]
    pub image_ref: String,
    /// This instance's unique ID (guid)
    #[serde(default)]
    pub instance_id: String,
    /// The revision number for this actor instance
    #[serde(default)]
    pub revision: i32,
    /// The maximum number of concurrent requests this instance can handle
    #[serde(default)]
    pub max_instances: u32,
}
