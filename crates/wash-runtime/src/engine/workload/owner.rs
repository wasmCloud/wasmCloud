use std::collections::HashMap;
use std::sync::Arc;

/// The object whose replicas share a workload's persistent state.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OwnerKind {
    Deployment,
    DaemonSet,
    ReplicaSet,
    Workload,
}

impl OwnerKind {
    /// Stable token for storage keys and metrics.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deployment => "deployment",
            Self::DaemonSet => "daemonset",
            Self::ReplicaSet => "replicaset",
            Self::Workload => "workload",
        }
    }
}

/// Logical workload identity, shared across replicas and revisions.
///
/// The namespace, kind, and name together identify the owner. This identity
/// reflects the orchestrator's input, not host-verified authorization.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WorkloadOwner {
    pub namespace: Arc<str>,
    pub kind: OwnerKind,
    pub name: Arc<str>,
}

impl WorkloadOwner {
    pub fn new(namespace: impl Into<Arc<str>>, kind: OwnerKind, name: impl Into<Arc<str>>) -> Self {
        Self {
            namespace: namespace.into(),
            kind,
            name: name.into(),
        }
    }
}

const DEPLOYMENT: &str = "runtime.wasmcloud.dev/workload-deployment";
const DAEMON_SET: &str = "runtime.wasmcloud.dev/workload-daemonset";
const REPLICA_SET: &str = "runtime.wasmcloud.dev/workload-replicaset-name";
const LEGACY_REPLICA_SET: &str = "runtime.wasmcloud.dev/workload-replicaset";

/// Interpret legacy orchestrator metadata once, before binding any plugins.
pub(crate) fn resolve_owner(
    namespace: &str,
    workload: &str,
    annotations: &HashMap<String, String>,
) -> anyhow::Result<WorkloadOwner> {
    for (key, kind) in [
        (DEPLOYMENT, OwnerKind::Deployment),
        (DAEMON_SET, OwnerKind::DaemonSet),
        (REPLICA_SET, OwnerKind::ReplicaSet),
        (LEGACY_REPLICA_SET, OwnerKind::ReplicaSet),
    ] {
        let Some(name) = annotations
            .get(key)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        anyhow::ensure!(
            name.len() <= 253
                && name.split('.').all(|label| {
                    label
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                        && label.starts_with(|c: char| c.is_ascii_alphanumeric())
                        && label.ends_with(|c: char| c.is_ascii_alphanumeric())
                }),
            "workload owner annotation {key} must name a Kubernetes object, not {name:?}"
        );
        return Ok(WorkloadOwner::new(namespace, kind, name));
    }
    Ok(WorkloadOwner::new(namespace, OwnerKind::Workload, workload))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn annotations(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn owner_precedence_and_blank_values() {
        for (pairs, kind, name) in [
            (
                vec![
                    (DEPLOYMENT, " dep\n"),
                    (DAEMON_SET, "ds"),
                    (REPLICA_SET, "rs"),
                ],
                OwnerKind::Deployment,
                "dep",
            ),
            (
                vec![(DAEMON_SET, " ds "), (REPLICA_SET, "rs")],
                OwnerKind::DaemonSet,
                "ds",
            ),
            (
                vec![
                    (DEPLOYMENT, " "),
                    (REPLICA_SET, " rs "),
                    (LEGACY_REPLICA_SET, "old"),
                ],
                OwnerKind::ReplicaSet,
                "rs",
            ),
            (
                vec![(REPLICA_SET, ""), (LEGACY_REPLICA_SET, " old ")],
                OwnerKind::ReplicaSet,
                "old",
            ),
            (
                vec![("app.kubernetes.io/name", "unrelated")],
                OwnerKind::Workload,
                "replica",
            ),
            (vec![], OwnerKind::Workload, "replica"),
        ] {
            assert_eq!(
                resolve_owner("team", "replica", &annotations(&pairs)).unwrap(),
                WorkloadOwner::new("team", kind, name)
            );
        }
    }

    #[test]
    fn invalid_selected_owner_fails_instead_of_falling_back() {
        for bad in [
            "Bad Name",
            "../other",
            "-dep",
            "dep-",
            "a/b",
            "a..b",
            "a.-b",
            "a-.b",
            &"a".repeat(254),
        ] {
            assert!(
                resolve_owner(
                    "team",
                    "replica",
                    &annotations(&[(DEPLOYMENT, bad), (REPLICA_SET, "valid")])
                )
                .is_err()
            );
        }
    }

    #[test]
    fn every_replica_and_revision_has_the_same_owner() {
        let a = resolve_owner(
            "team",
            "dep-aaa-1",
            &annotations(&[(DEPLOYMENT, "dep"), (REPLICA_SET, "dep-aaa")]),
        )
        .unwrap();
        let b = resolve_owner(
            "team",
            "dep-bbb-2",
            &annotations(&[(DEPLOYMENT, "dep"), (REPLICA_SET, "dep-bbb")]),
        )
        .unwrap();
        assert_eq!(a, b);
        assert_ne!(a, WorkloadOwner::new("team", OwnerKind::Workload, "dep"));
        assert_ne!(a, WorkloadOwner::new("other", OwnerKind::Deployment, "dep"));
    }
}
