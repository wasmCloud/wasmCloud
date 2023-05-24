use std::collections::HashMap;
use wasmbus_rpc::core::ActorLinks;

use serde::Deserialize;

/// JSON Output of the `wash start` command
#[derive(Debug, Deserialize)]
pub struct StartCommandJsonOutput {
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
pub struct StopCommandJsonOutput {
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
