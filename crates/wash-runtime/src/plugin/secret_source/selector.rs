//! Workload selectors used by secret grants.

use super::RequestingWorkload;

/// Matches a namespace and optional stable workload name. Under the operator,
/// the name identifies the owning WorkloadDeployment, WorkloadDaemonSet, or
/// WorkloadReplicaSet. Both fields come from the start request, so grants
/// trust its sender.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkloadSelector {
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl WorkloadSelector {
    /// Every workload in `namespace`.
    pub fn namespace(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: None,
        }
    }

    /// The workload called `name` in `namespace`.
    pub fn workload(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: Some(name.into()),
        }
    }

    pub(super) fn admits(&self, workload: &RequestingWorkload) -> bool {
        self.namespace == workload.namespace
            && self.name.as_ref().is_none_or(|name| *name == workload.name)
    }
}
