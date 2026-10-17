//! The slice of wasmCloud's host NATS API the bench uses.
//!
//! Messages are the protobuf types in `proto/wasmcloud/runtime/v2`, encoded
//! as pbjson JSON: camelCase fields, enums by name, 64-bit integers as
//! strings. These are hand-written mirrors rather than the generated types so
//! the bench depends on no host crate. Only fields the proto defines are sent:
//! the generated deserializers refuse unknown ones.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context as _, Result};
use async_nats::RequestErrorKind;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

pub const HOST_PREFIX: &str = "runtime.host";
pub const HEARTBEAT_SUBJECT: &str = "runtime.operator.heartbeat.*";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkloadStartRequest {
    pub workload: Workload,
    pub workload_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workload {
    pub namespace: String,
    pub name: String,
    pub annotations: BTreeMap<String, String>,
    pub wit_world: WitWorld,
    pub volumes: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WitWorld {
    pub components: Vec<Component>,
    pub host_interfaces: Vec<WitInterface>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub name: String,
    pub image: String,
    pub pool_size: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_invocations: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<i32>,
    pub image_pull_policy: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WitInterface {
    pub namespace: String,
    pub package: String,
    pub interfaces: Vec<String>,
    pub version: String,
    pub config: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkloadIdRequest {
    pub workload_id: String,
}

/// The reply to `workload.start`, `workload.status` and `workload.stop`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkloadResponse {
    pub workload_status: Option<WorkloadStatus>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkloadStatus {
    pub workload_id: String,
    pub workload_state: WorkloadState,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WorkloadState {
    #[default]
    Unspecified,
    Starting,
    Running,
    Completed,
    Stopping,
    Error,
    NotFound,
    Unknown(String),
}

impl WorkloadState {
    fn from_name(name: &str) -> Self {
        match name {
            "WORKLOAD_STATE_UNSPECIFIED" => Self::Unspecified,
            "WORKLOAD_STATE_STARTING" => Self::Starting,
            "WORKLOAD_STATE_RUNNING" => Self::Running,
            "WORKLOAD_STATE_COMPLETED" => Self::Completed,
            "WORKLOAD_STATE_STOPPING" => Self::Stopping,
            "WORKLOAD_STATE_ERROR" => Self::Error,
            "WORKLOAD_STATE_NOT_FOUND" => Self::NotFound,
            other => Self::Unknown(other.to_string()),
        }
    }

    fn from_number(n: i64) -> Self {
        match n {
            0 => Self::Unspecified,
            1 => Self::Starting,
            2 => Self::Running,
            3 => Self::Completed,
            4 => Self::Stopping,
            5 => Self::Error,
            6 => Self::NotFound,
            other => Self::Unknown(other.to_string()),
        }
    }
}

/// pbjson writes enums by name but reads either form; accept both too.
impl<'de> Deserialize<'de> for WorkloadState {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Name(String),
            Number(i64),
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Name(name) => Self::from_name(&name),
            Raw::Number(n) => Self::from_number(n),
        })
    }
}

/// The heartbeat fields the report uses; everything else is ignored.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HostHeartbeat {
    pub id: String,
    pub hostname: String,
    pub friendly_name: String,
    pub version: String,
    pub os_arch: String,
    pub os_name: String,
    pub os_kernel: String,
    #[serde(deserialize_with = "u64_from_any")]
    pub system_memory_total: u64,
    #[serde(deserialize_with = "u64_from_any")]
    pub system_memory_free: u64,
    #[serde(deserialize_with = "u64_from_any")]
    pub component_count: u64,
    #[serde(deserialize_with = "u64_from_any")]
    pub workload_count: u64,
    pub http_port: u32,
}

/// proto3 JSON writes `uint64` as a string; take a number as well.
fn u64_from_any<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Number(u64),
        Text(String),
    }
    match Raw::deserialize(deserializer)? {
        Raw::Number(n) => Ok(n),
        Raw::Text(text) => text.parse().map_err(serde::de::Error::custom),
    }
}

/// How a request to the host failed, in the categories the bench counts.
#[derive(Debug)]
pub enum RpcError {
    /// No reply in time. The host drops commands past its admission queue
    /// without answering, so this is what a drop looks like from outside.
    Timeout,
    /// Nothing subscribed: the host is not (or no longer) listening.
    NoResponders,
    Other(anyhow::Error),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => f.write_str("no reply before the timeout"),
            Self::NoResponders => f.write_str("no responders"),
            Self::Other(e) => write!(f, "{e:#}"),
        }
    }
}

/// One host's command subjects.
#[derive(Clone)]
pub struct HostClient {
    nats: async_nats::Client,
    host_id: String,
}

impl HostClient {
    pub fn new(nats: async_nats::Client, host_id: String) -> Self {
        Self { nats, host_id }
    }

    pub fn subject(&self, command: &str) -> String {
        format!("{HOST_PREFIX}.{}.{command}", self.host_id)
    }

    async fn request(
        &self,
        command: &str,
        payload: Vec<u8>,
        timeout: Duration,
    ) -> Result<bytes::Bytes, RpcError> {
        let request = async_nats::Request::new()
            .payload(payload.into())
            .timeout(Some(timeout));
        match self.nats.send_request(self.subject(command), request).await {
            Ok(message) => Ok(message.payload),
            Err(e) => Err(match e.kind() {
                RequestErrorKind::TimedOut => RpcError::Timeout,
                RequestErrorKind::NoResponders => RpcError::NoResponders,
                _ => RpcError::Other(anyhow::Error::new(e).context(command.to_string())),
            }),
        }
    }

    async fn rpc<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        command: &str,
        request: &Req,
        timeout: Duration,
    ) -> Result<Resp, RpcError> {
        let payload = serde_json::to_vec(request)
            .map_err(|e| RpcError::Other(anyhow::Error::new(e).context("encoding request")))?;
        let reply = self.request(command, payload, timeout).await?;
        serde_json::from_slice(&reply).map_err(|e| {
            RpcError::Other(anyhow::anyhow!(
                "{command}: undecodable reply ({e}): {}",
                String::from_utf8_lossy(&reply)
            ))
        })
    }

    /// The host subscribes to its API after it starts, so a command sent
    /// straight away can race it. Ask something harmless until it answers.
    pub async fn wait_ready(&self, limit: Duration) -> Result<()> {
        let probe = WorkloadIdRequest {
            workload_id: "bench-scale-probe".to_string(),
        };
        tokio::time::timeout(limit, async {
            loop {
                let answered: Result<WorkloadResponse, _> = self
                    .rpc("workload.status", &probe, Duration::from_secs(2))
                    .await;
                if answered.is_ok() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .with_context(|| format!("host {} did not answer within {limit:?}", self.host_id))
    }

    pub async fn heartbeat(&self, timeout: Duration) -> Result<HostHeartbeat, RpcError> {
        let reply = self.request("heartbeat", Vec::new(), timeout).await?;
        serde_json::from_slice(&reply)
            .map_err(|e| RpcError::Other(anyhow::anyhow!("undecodable heartbeat: {e}")))
    }

    pub async fn start(
        &self,
        request: &WorkloadStartRequest,
        timeout: Duration,
    ) -> Result<WorkloadStatus, RpcError> {
        let response: WorkloadResponse = self.rpc("workload.start", request, timeout).await?;
        status_of(response)
    }

    pub async fn status(
        &self,
        workload_id: &str,
        timeout: Duration,
    ) -> Result<WorkloadStatus, RpcError> {
        let request = WorkloadIdRequest {
            workload_id: workload_id.to_string(),
        };
        let response: WorkloadResponse = self.rpc("workload.status", &request, timeout).await?;
        status_of(response)
    }

    pub async fn stop(
        &self,
        workload_id: &str,
        timeout: Duration,
    ) -> Result<WorkloadStatus, RpcError> {
        let request = WorkloadIdRequest {
            workload_id: workload_id.to_string(),
        };
        let response: WorkloadResponse = self.rpc("workload.stop", &request, timeout).await?;
        status_of(response)
    }
}

fn status_of(response: WorkloadResponse) -> Result<WorkloadStatus, RpcError> {
    response
        .workload_status
        .ok_or_else(|| RpcError::Other(anyhow::anyhow!("reply carries no workloadStatus")))
}

/// The `wasi:http/incoming-handler` binding that routes `host_header` to a
/// workload. The host matches the request's Host header against it, so it has
/// to be an RFC 1123 hostname.
pub fn http_interface(host_header: &str) -> WitInterface {
    WitInterface {
        namespace: "wasi".to_string(),
        package: "http".to_string(),
        interfaces: vec!["incoming-handler".to_string()],
        version: "0.2.2".to_string(),
        config: BTreeMap::from([("host".to_string(), host_header.to_string())]),
    }
}

/// A start request for one HTTP workload of `image`.
pub fn start_request(
    workload_id: &str,
    name: &str,
    image: &str,
    pool_size: i32,
    host_header: &str,
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Workload {
            namespace: "bench-scale".to_string(),
            name: name.to_string(),
            annotations: BTreeMap::new(),
            wit_world: WitWorld {
                components: vec![Component {
                    name: "handler".to_string(),
                    image: image.to_string(),
                    pool_size,
                    max_invocations: None,
                    max_concurrency: None,
                    image_pull_policy: "IMAGE_PULL_POLICY_IF_NOT_PRESENT".to_string(),
                }],
                host_interfaces: vec![http_interface(host_header)],
            },
            volumes: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_request_uses_proto_json_names() {
        let request = start_request(
            "w-1",
            "w1",
            "127.0.0.1:5000/bench/h:abc",
            1,
            "w1.warm.scale.test",
        );
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["workloadId"], "w-1");
        let workload = &json["workload"];
        assert_eq!(workload["namespace"], "bench-scale");
        assert_eq!(workload["name"], "w1");
        assert!(workload["annotations"].as_object().unwrap().is_empty());
        assert!(workload["volumes"].as_array().unwrap().is_empty());
        let component = &workload["witWorld"]["components"][0];
        assert_eq!(component["name"], "handler");
        assert_eq!(component["image"], "127.0.0.1:5000/bench/h:abc");
        assert_eq!(component["poolSize"], 1);
        assert_eq!(
            component["imagePullPolicy"],
            "IMAGE_PULL_POLICY_IF_NOT_PRESENT"
        );
        assert!(component.get("maxInvocations").is_none());
        let interface = &workload["witWorld"]["hostInterfaces"][0];
        assert_eq!(interface["namespace"], "wasi");
        assert_eq!(interface["package"], "http");
        assert_eq!(interface["interfaces"][0], "incoming-handler");
        assert_eq!(interface["version"], "0.2.2");
        assert_eq!(interface["config"]["host"], "w1.warm.scale.test");
        // Only proto fields: pbjson's deserializers refuse anything else.
        let keys: Vec<&String> = component.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["image", "imagePullPolicy", "name", "poolSize"]);
    }

    #[test]
    fn status_request_shape() {
        let json = serde_json::to_value(WorkloadIdRequest {
            workload_id: "x".into(),
        })
        .unwrap();
        assert_eq!(json, serde_json::json!({"workloadId": "x"}));
    }

    #[test]
    fn decodes_start_response_by_name_and_number() {
        let response: WorkloadResponse = serde_json::from_str(
            r#"{"workloadStatus":{"workloadId":"w-1","workloadState":"WORKLOAD_STATE_ERROR","message":"maximum concurrent limit of 1000 for core instances reached"}}"#,
        )
        .unwrap();
        let status = response.workload_status.unwrap();
        assert_eq!(status.workload_id, "w-1");
        assert_eq!(status.workload_state, WorkloadState::Error);
        assert!(status.message.contains("core instances"));

        let response: WorkloadResponse =
            serde_json::from_str(r#"{"workloadStatus":{"workloadState":2}}"#).unwrap();
        assert_eq!(
            response.workload_status.unwrap().workload_state,
            WorkloadState::Running
        );

        // pbjson leaves out default values, so an empty status is valid.
        let response: WorkloadResponse = serde_json::from_str(r#"{"workloadStatus":{}}"#).unwrap();
        assert_eq!(
            response.workload_status.unwrap().workload_state,
            WorkloadState::Unspecified
        );
    }

    #[test]
    fn decodes_heartbeat_with_string_u64s() {
        let heartbeat: HostHeartbeat = serde_json::from_str(
            r#"{"id":"NABC","hostname":"box","version":"2.10.3","systemMemoryTotal":"8000000000",
                "systemMemoryFree":123,"componentCount":"4","workloadCount":"4","httpPort":8080,
                "labels":{"a":"b"},"startedAt":"2026-01-01T00:00:00Z","imports":[]}"#,
        )
        .unwrap();
        assert_eq!(heartbeat.id, "NABC");
        assert_eq!(heartbeat.version, "2.10.3");
        assert_eq!(heartbeat.system_memory_total, 8_000_000_000);
        assert_eq!(heartbeat.system_memory_free, 123);
        assert_eq!(heartbeat.workload_count, 4);
        assert_eq!(heartbeat.component_count, 4);
        assert_eq!(heartbeat.http_port, 8080);

        let empty: HostHeartbeat = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.workload_count, 0);
    }
}
