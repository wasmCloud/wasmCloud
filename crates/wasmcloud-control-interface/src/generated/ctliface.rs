use rmp_serde::{Deserializer, Serializer};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct ProviderAuctionRequest {
    #[serde(rename = "provider_ref")]
    pub provider_ref: String,
    #[serde(rename = "link_name")]
    pub link_name: String,
    #[serde(rename = "constraints")]
    pub constraints: std::collections::HashMap<String, String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct ProviderAuctionAck {
    #[serde(rename = "provider_ref")]
    pub provider_ref: String,
    #[serde(rename = "link_name")]
    pub link_name: String,
    #[serde(rename = "host_id")]
    pub host_id: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct ActorAuctionRequest {
    #[serde(rename = "actor_ref")]
    pub actor_ref: String,
    #[serde(rename = "constraints")]
    pub constraints: std::collections::HashMap<String, String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct ActorAuctionAck {
    #[serde(rename = "actor_ref")]
    pub actor_ref: String,
    #[serde(rename = "constraints")]
    pub constraints: std::collections::HashMap<String, String>,
    #[serde(rename = "host_id")]
    pub host_id: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct StartActorCommand {
    #[serde(rename = "actor_ref")]
    pub actor_ref: String,
    #[serde(rename = "host_id")]
    pub host_id: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct StartActorAck {
    #[serde(rename = "host_id")]
    pub host_id: String,
    #[serde(rename = "actor_ref")]
    pub actor_ref: String,
    #[serde(rename = "failure")]
    pub failure: Option<String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct StartProviderCommand {
    #[serde(rename = "host_id")]
    pub host_id: String,
    #[serde(rename = "provider_ref")]
    pub provider_ref: String,
    #[serde(rename = "link_name")]
    pub link_name: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct StartProviderAck {
    #[serde(rename = "host_id")]
    pub host_id: String,
    #[serde(rename = "provider_ref")]
    pub provider_ref: String,
    #[serde(rename = "failure")]
    pub failure: Option<String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct StopActorCommand {
    #[serde(rename = "host_id")]
    pub host_id: String,
    #[serde(rename = "actor_ref")]
    pub actor_ref: String,
    #[serde(rename = "count")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u16>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct StopProviderCommand {
    #[serde(rename = "host_id")]
    pub host_id: String,
    #[serde(rename = "provider_ref")]
    pub provider_ref: String,
    #[serde(rename = "link_name")]
    pub link_name: String,
    #[serde(rename = "contract_id")]
    pub contract_id: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct UpdateActorCommand {
    #[serde(rename = "host_id")]
    pub host_id: String,
    #[serde(rename = "actor_id")]
    pub actor_id: String,
    #[serde(rename = "new_actor_ref")]
    pub new_actor_ref: String,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct UpdateActorAck {
    #[serde(rename = "accepted")]
    pub accepted: bool,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct StopActorAck {
    #[serde(rename = "failure")]
    pub failure: Option<String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct StopProviderAck {
    #[serde(rename = "failure")]
    pub failure: Option<String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct LinkDefinitionList {
    #[serde(rename = "links")]
    pub links: Vec<LinkDefinition>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct LinkDefinition {
    #[serde(rename = "actor_id")]
    pub actor_id: String,
    #[serde(rename = "provider_id")]
    pub provider_id: String,
    #[serde(rename = "link_name")]
    pub link_name: String,
    #[serde(rename = "contract_id")]
    pub contract_id: String,
    #[serde(rename = "values")]
    pub values: std::collections::HashMap<String, String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct HostList {
    #[serde(rename = "hosts")]
    pub hosts: Vec<Host>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct Host {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "uptime")]
    pub uptime_seconds: u64,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct ClaimsList {
    #[serde(rename = "claims")]
    pub claims: Vec<Claims>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct Claims {
    #[serde(rename = "values")]
    pub values: std::collections::HashMap<String, String>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct HostInventory {
    #[serde(rename = "host_id")]
    pub host_id: String,
    #[serde(rename = "labels")]
    pub labels: std::collections::HashMap<String, String>,
    #[serde(rename = "actors")]
    pub actors: Vec<ActorDescription>,
    #[serde(rename = "providers")]
    pub providers: Vec<ProviderDescription>,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct ActorDescription {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "image_ref")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<String>,
    #[serde(rename = "name")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "revision")]
    pub revision: i32,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Default, Clone)]
pub struct ProviderDescription {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "link_name")]
    pub link_name: String,
    #[serde(rename = "image_ref")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_ref: Option<String>,
    #[serde(rename = "name")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "revision")]
    pub revision: i32,
}

/// The standard function for serializing codec structs into a format that can be
/// used for message exchange between actor and host. Use of any other function to
/// serialize could result in breaking incompatibilities.
pub(crate) fn serialize<T>(
    item: T,
) -> ::std::result::Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>>
where
    T: Serialize,
{
    let mut buf = Vec::new();
    item.serialize(&mut Serializer::new(&mut buf).with_struct_map())?;
    Ok(buf)
}

/// The standard function for de-serializing codec structs from a format suitable
/// for message exchange between actor and host. Use of any other function to
/// deserialize could result in breaking incompatibilities.
pub(crate) fn deserialize<'de, T: Deserialize<'de>>(
    buf: &[u8],
) -> ::std::result::Result<T, Box<dyn std::error::Error + Send + Sync>> {
    let mut de = Deserializer::new(Cursor::new(buf));
    match Deserialize::deserialize(&mut de) {
        Ok(t) => Ok(t),
        Err(e) => Err(format!("Failed to de-serialize: {}", e).into()),
    }
}
