use std::collections::HashMap;
use wasmbus_rpc::core::ActorLinks;

use serde::Deserialize;
use wasmcloud_control_interface::{GetClaimsResponse, Host, HostInventory};

/// JSON Output of the `wash start` command
#[derive(Debug, Deserialize)]
pub struct StartCommandOutput {
    pub actor_id: Option<String>,
    pub actor_ref: Option<String>,

    pub provider_id: Option<String>,
    pub provider_ref: Option<String>,
    pub contract_id: Option<String>,
    pub link_name: Option<String>,

    pub host_id: Option<String>,
    pub success: bool,
}

/// JSON Output representation of the `wash stop` command
#[derive(Debug, Deserialize)]
pub struct StopCommandOutput {
    pub host_id: Option<String>,
    pub result: String,

    pub actor_ref: Option<String>,
    pub actor_id: Option<String>,

    pub provider_id: Option<String>,
    pub link_name: Option<String>,
    pub contract_id: Option<String>,
    pub provider_ref: Option<String>,

    pub success: bool,
}

/// JSON output representation of the `wash link query` command
#[derive(Debug, Deserialize)]
pub struct LinkQueryOutput {
    pub links: Vec<HashMap<String, ActorLinks>>,
    pub success: bool,
}

/// JSON output representation of the `wash get hosts` command
#[derive(Debug, Clone, Deserialize)]
pub struct GetHostsOutput {
    pub success: bool,
    pub hosts: Vec<Host>,
}

/// JSON output representation of the `wash get inventory` command
#[derive(Debug, Clone, Deserialize)]
pub struct GetHostInventoryOutput {
    pub success: bool,
    pub inventory: HostInventory,
}

/// JSON output representation of the `wash get claims` command
#[derive(Debug, Deserialize)]
pub struct GetClaimsOutput {
    pub claims: GetClaimsResponse,
    pub success: bool,
}
