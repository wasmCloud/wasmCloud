use std::collections::HashMap;

use serde::Deserialize;
use wasmcloud_control_interface::{Host, HostInventory, Link};

use wadm_types::api::ModelSummary;
use wadm_types::validation::ValidationFailure;

/// JSON Output of the `wash start` command
#[derive(Debug, Deserialize)]
pub struct StartCommandOutput {
    pub component_id: Option<String>,
    pub component_ref: Option<String>,

    pub provider_id: Option<String>,
    pub provider_ref: Option<String>,

    pub host_id: Option<String>,
    pub success: bool,
}

/// JSON Output representation of the `wash stop` command
#[derive(Debug, Deserialize)]
pub struct StopCommandOutput {
    pub host_id: Option<String>,
    pub result: String,

    pub component_ref: Option<String>,
    pub component_id: Option<String>,

    pub provider_id: Option<String>,
    pub provider_ref: Option<String>,

    pub success: bool,
}

/// JSON output representation of the `wash link query` command
#[derive(Debug, Deserialize)]
pub struct LinkQueryCommandOutput {
    pub links: Vec<Link>,
    pub success: bool,
}

/// JSON output representation of the `wash get hosts` command
#[derive(Debug, Clone, Deserialize)]
pub struct GetHostsCommandOutput {
    pub success: bool,
    pub hosts: Vec<Host>,
}

/// JSON output representation of the `wash get inventory` command
#[derive(Debug, Clone, Deserialize)]
pub struct GetHostInventoriesCommandOutput {
    pub success: bool,
    pub inventories: Vec<HostInventory>,
}

/// JSON output representation of the `wash get claims` command
#[derive(Debug, Deserialize)]
pub struct GetClaimsCommandOutput {
    pub claims: Vec<HashMap<String, String>>,
    pub success: bool,
}

/// JSON output representation of the `wash dev` command
#[derive(Debug, Deserialize)]
pub struct DevCommandOutput {
    pub success: bool,
}

/// JSON output representation of the `wash dev` command
#[derive(Debug, Deserialize)]
pub struct ScaleCommandOutput {
    pub success: bool,
    pub result: String,
}

/// JSON output representation of the `wash call` command
#[derive(Debug, Deserialize)]
pub struct CallCommandOutput {
    pub success: bool,
    pub response: serde_json::Value,
}

/// JSON output representation of the `wash pull` command
#[derive(Debug, Deserialize)]
pub struct PullCommandOutput {
    pub success: bool,
    pub file: String,
}

/// JSON output representation of the `wash label` command
#[derive(Debug, Deserialize)]
pub struct LabelHostCommandOutput {
    pub success: bool,
    pub deleted: bool,
    pub processed: Vec<(String, String)>,
}

/// JSON output representation of the `wash up` command
#[derive(Debug, Clone, Deserialize)]
pub struct UpCommandOutput {
    pub success: bool,
    pub kill_cmd: String,
    pub wasmcloud_log: String,
    pub nats_url: String,
    pub deployed_wadm_manifest_path: Option<String>,
}

/// JSON output representation of the `wash app validate` command
#[derive(Debug, Deserialize)]
pub struct AppValidateOutput {
    pub valid: bool,
    pub warnings: Vec<ValidationFailure>,
    pub errors: Vec<ValidationFailure>,
}

/// JSON Output representation of the `wash app deploy` command
#[derive(Debug, Deserialize)]
pub struct AppDeployCommandOutput {
    pub success: bool,
    pub deployed: bool,
    pub model_name: String,
    pub model_version: String,
}

/// JSON Output representation of the `wash app list` command
#[derive(Debug, Deserialize)]
pub struct AppListCommandOutput {
    pub success: bool,
    pub applications: Vec<ModelSummary>,
}

/// JSON Output representation of the `wash app get` command
#[derive(Debug, Deserialize)]
pub struct AppGetCommandOutput {
    pub success: bool,
    pub applications: Vec<ModelSummary>,
}

/// JSON Output representation of the `wash app undeploy` command
#[derive(Debug, Deserialize)]
pub struct AppUndeployCommandOutput {
    pub success: bool,
}

/// JSON Output representation of the `wash app delete` command
#[derive(Debug, Deserialize)]
pub struct AppDeleteCommandOutput {
    pub success: bool,
}
