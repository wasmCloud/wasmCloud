//! The host NATS API, over the generated `wasmcloud.runtime.v2` types: JSON
//! request and reply on `runtime.host.<id>.<command>`.

use std::time::Duration;

use async_nats::RequestErrorKind;
use serde::Serialize;
use serde::de::DeserializeOwned;
use wash_runtime::washlet::rpc_subject;
use wash_runtime::washlet::types::v2::{
    Component, HostHeartbeat, ImagePullPolicy, WitInterface, WitWorld, Workload,
    WorkloadStartRequest, WorkloadStartResponse, WorkloadState, WorkloadStatus,
    WorkloadStatusRequest, WorkloadStatusResponse, WorkloadStopRequest, WorkloadStopResponse,
};

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

/// A workload's state and the host's message about it.
pub struct Status {
    pub state: WorkloadState,
    pub message: String,
}

impl From<Option<WorkloadStatus>> for Status {
    fn from(status: Option<WorkloadStatus>) -> Self {
        let status = status.unwrap_or_default();
        Self {
            state: WorkloadState::try_from(status.workload_state)
                .unwrap_or(WorkloadState::Unspecified),
            message: status.message,
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

    async fn rpc<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        command: &str,
        request: &Req,
        timeout: Duration,
    ) -> Result<Resp, RpcError> {
        let payload = serde_json::to_vec(request)
            .map_err(|e| RpcError::Other(anyhow::Error::new(e).context("encoding request")))?;
        let request = async_nats::Request::new()
            .payload(payload.into())
            .timeout(Some(timeout));
        let reply = match self
            .nats
            .send_request(rpc_subject(&self.host_id, command), request)
            .await
        {
            Ok(message) => message.payload,
            Err(e) => {
                return Err(match e.kind() {
                    RequestErrorKind::TimedOut => RpcError::Timeout,
                    RequestErrorKind::NoResponders => RpcError::NoResponders,
                    _ => RpcError::Other(anyhow::Error::new(e).context(command.to_string())),
                });
            }
        };
        serde_json::from_slice(&reply)
            .map_err(|e| RpcError::Other(anyhow::anyhow!("{command}: undecodable reply ({e})")))
    }

    pub async fn heartbeat(&self, timeout: Duration) -> Result<HostHeartbeat, RpcError> {
        self.rpc("heartbeat", &(), timeout).await
    }

    pub async fn start(
        &self,
        request: &WorkloadStartRequest,
        timeout: Duration,
    ) -> Result<Status, RpcError> {
        let response: WorkloadStartResponse = self.rpc("workload.start", request, timeout).await?;
        Ok(response.workload_status.into())
    }

    pub async fn status(&self, workload_id: &str, timeout: Duration) -> Result<Status, RpcError> {
        let request = WorkloadStatusRequest {
            workload_id: workload_id.to_string(),
        };
        let response: WorkloadStatusResponse =
            self.rpc("workload.status", &request, timeout).await?;
        Ok(response.workload_status.into())
    }

    pub async fn stop(&self, workload_id: &str, timeout: Duration) -> Result<Status, RpcError> {
        let request = WorkloadStopRequest {
            workload_id: workload_id.to_string(),
        };
        let response: WorkloadStopResponse = self.rpc("workload.stop", &request, timeout).await?;
        Ok(response.workload_status.into())
    }
}

/// A start request for one HTTP workload of `image`, routed by `host_header`.
pub fn start_request(
    workload_id: &str,
    name: &str,
    image: &str,
    pool_size: i32,
    host_header: &str,
) -> WorkloadStartRequest {
    WorkloadStartRequest {
        workload_id: workload_id.to_string(),
        workload: Some(Workload {
            namespace: "bench-scale".to_string(),
            name: name.to_string(),
            wit_world: Some(WitWorld {
                components: vec![Component {
                    name: "handler".to_string(),
                    image: image.to_string(),
                    pool_size,
                    image_pull_policy: ImagePullPolicy::IfNotPresent.into(),
                    ..Default::default()
                }],
                host_interfaces: vec![WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: vec!["handler".to_string()],
                    version: "0.3.0".to_string(),
                    config: [("host".to_string(), host_header.to_string())].into(),
                    ..Default::default()
                }],
            }),
            ..Default::default()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_request_uses_proto_json_names() {
        let request = start_request("w-1", "w1", "127.0.0.1:5000/h:abc", 1, "w1.warm.test");
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["workloadId"], "w-1");
        let component = &json["workload"]["witWorld"]["components"][0];
        assert_eq!(component["image"], "127.0.0.1:5000/h:abc");
        assert_eq!(component["poolSize"], 1);
        assert_eq!(
            component["imagePullPolicy"],
            "IMAGE_PULL_POLICY_IF_NOT_PRESENT"
        );
        let interface = &json["workload"]["witWorld"]["hostInterfaces"][0];
        assert_eq!(interface["config"]["host"], "w1.warm.test");
    }

    #[test]
    fn statuses_decode_by_name_or_number_and_default_when_absent() {
        let named: WorkloadStartResponse = serde_json::from_str(
            r#"{"workloadStatus":{"workloadState":"WORKLOAD_STATE_ERROR","message":"pool"}}"#,
        )
        .unwrap();
        let status = Status::from(named.workload_status);
        assert_eq!(status.state, WorkloadState::Error);
        assert_eq!(status.message, "pool");
        let numbered: WorkloadStartResponse =
            serde_json::from_str(r#"{"workloadStatus":{"workloadState":2}}"#).unwrap();
        assert_eq!(
            Status::from(numbered.workload_status).state,
            WorkloadState::Running
        );
        assert_eq!(Status::from(None).state, WorkloadState::Unspecified);
    }
}
