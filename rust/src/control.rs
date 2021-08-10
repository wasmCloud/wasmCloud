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
    context::Context, deserialize, serialize, Message, MessageDispatch, RpcError, RpcResult,
    SendOpts, Transport,
};

pub const SMITHY_VERSION: &str = "1.0";

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActorAuctionAck {
    #[serde(default)]
    pub actor_ref: String,
    pub constraints: ConstraintMap,
    #[serde(default)]
    pub host_id: String,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActorAuctionRequest {
    #[serde(default)]
    pub actor_ref: String,
    pub constraints: ConstraintMap,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CacheAck {
    #[serde(default)]
    pub accepted: bool,
}

pub type ClaimsList = Vec<ClaimsMap>;

pub type ClaimsMap = std::collections::HashMap<String, String>;

pub type ConstraintMap = std::collections::HashMap<String, String>;

/// response to get_claims
#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GetClaimsResponse {
    pub claims: ClaimsList,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Host {
    #[serde(default)]
    pub id: String,
    /// uptime in seconds
    pub uptime_seconds: u64,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostInventory {
    pub actors: ActorDescriptions,
    #[serde(default)]
    pub host_id: String,
    pub labels: LabelsMap,
    pub providers: ProviderDescriptions,
}

pub type HostList = Vec<Host>;

pub type LabelsMap = std::collections::HashMap<String, String>;

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LinkDefinitionList {
    pub links: wasmbus_rpc::core::ActorLinks,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderAuctionAck {
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub provider_ref: String,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderAuctionRequest {
    pub constraints: ConstraintMap,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub provider_ref: String,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StartActorAck {
    #[serde(default)]
    pub actor_ref: String,
    /// optional failure message
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(default)]
    pub host_id: String,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StartActorCommand {
    #[serde(default)]
    pub actor_ref: String,
    #[serde(default)]
    pub host_id: String,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StartProviderAck {
    /// optional failure message
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub provider_ref: String,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StartProviderCommand {
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub link_name: String,
    #[serde(default)]
    pub provider_ref: String,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StopActorAck {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StopActorCommand {
    #[serde(default)]
    pub actor_ref: String,
    /// optional count
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u16>,
    #[serde(default)]
    pub host_id: String,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StopProviderAck {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UpdateActorAck {
    #[serde(default)]
    pub accepted: bool,
}

#[derive(Default, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UpdateActorCommand {
    #[serde(default)]
    pub actor_id: String,
    #[serde(default)]
    pub host_id: String,
    #[serde(default)]
    pub new_actor_ref: String,
}
