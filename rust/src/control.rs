// This file is generated automatically using wasmcloud-weld and smithy model definitions
//

#![allow(clippy::ptr_arg)]
#[allow(unused_imports)]
use async_trait::async_trait;
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use std::{borrow::Cow, string::ToString};
#[allow(unused_imports)]
use wasmbus_rpc::{
    deserialize, serialize, Context, Message, MessageDispatch, RpcError, RpcResult, SendOpts,
    Transport,
};

pub const SMITHY_VERSION: &str = "1.0";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorAuctionAck {
    #[serde(default)]
    pub actor_ref: String,
    pub constraints: ConstraintMap,
    #[serde(default)]
    pub host_id: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorAuctionRequest {
    #[serde(default)]
    pub actor_ref: String,
    pub constraints: ConstraintMap,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActorDescription {
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub revision: i32,
}

pub type ActorDescriptions = Vec<ActorDescription>;

pub type ClaimsList = Vec<ClaimsMap>;

pub type ClaimsMap = std::collections::HashMap<String, String>;

pub type ConstraintMap = std::collections::HashMap<String, String>;

/// Standard response for control interface operations
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CtlOperationAck {
    #[serde(default)]
    pub accepted: bool,
    #[serde(default)]
    pub error: String,
}

/// response to get_claims
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GetClaimsResponse {
    pub claims: ClaimsList,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Host {
    #[serde(default)]
    pub id: String,
    /// uptime in seconds
    pub uptime_seconds: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostInventory {
    pub actors: ActorDescriptions,
    #[serde(default)]
    pub host_id: String,
    pub labels: LabelsMap,
    pub providers: ProviderDescriptions,
}

pub type HostList = Vec<Host>;

pub type LabelsMap = std::collections::HashMap<String, String>;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LinkDefinitionList {
    pub links: wasmbus_rpc::core::ActorLinks,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderAuctionAck {
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub provider_ref: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderAuctionRequest {
    pub constraints: ConstraintMap,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub provider_ref: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderDescription {
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<String>,
    #[serde(default)]
    pub link_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub revision: i32,
}

pub type ProviderDescriptions = Vec<ProviderDescription>;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartActorCommand {
    #[serde(default)]
    pub actor_ref: String,
    #[serde(default)]
    pub host_id: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StartProviderCommand {
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub provider_ref: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StopActorCommand {
    #[serde(default)]
    pub actor_ref: String,
    /// optional count
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u16>,
    #[serde(default)]
    pub host_id: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StopProviderCommand {
    #[serde(default)]
    pub contract_id: String,
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub provider_ref: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateActorCommand {
    #[serde(default)]
    pub actor_id: String,
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub new_actor_ref: String,
}
