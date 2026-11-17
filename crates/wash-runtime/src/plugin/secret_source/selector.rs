//! Workload selectors used by secret grants.

use super::RequestingWorkload;
use crate::types::WorkloadOwner;

/// Matches workloads in a namespace: every one, the replicas of one owner, or
/// those that keep one name.
///
/// With `kind`, it names an owner as the control plane verified it, pinned to
/// one object when `uid` is given. Without `kind`, `name` is the name a
/// workload keeps across replicas, which its start request chooses, so any
/// workload in the namespace can claim it. That form is for plugin secret
/// grants that predate owners; name the owner's kind instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkloadSelector {
    pub namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Unchecked {
    namespace: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    uid: Option<String>,
}

impl<'de> serde::Deserialize<'de> for WorkloadSelector {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let Unchecked {
            namespace,
            name,
            kind,
            uid,
        } = Unchecked::deserialize(d)?;
        let selector = Self {
            namespace,
            name,
            kind,
            uid,
        };
        selector.check().map_err(serde::de::Error::custom)?;
        Ok(selector)
    }
}

impl WorkloadSelector {
    /// Every workload in `namespace`.
    pub fn namespace(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: None,
            kind: None,
            uid: None,
        }
    }

    /// The workloads that keep the name `name` in `namespace`. See the type's
    /// documentation for why [`Self::owner`] is the safer grant.
    pub fn workload(namespace: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            name: Some(name.into()),
            kind: None,
            uid: None,
        }
    }

    /// The replicas of `owner` in `namespace`, by kind, name and UID.
    pub fn owner(namespace: impl Into<String>, owner: &WorkloadOwner) -> Self {
        Self {
            namespace: namespace.into(),
            name: Some(owner.name().to_string()),
            kind: Some(owner.kind().to_string()),
            uid: Some(owner.uid().to_string()),
        }
    }

    /// Whether this matches by a name the start request chooses rather than
    /// by owner, for a caller that refuses that form.
    pub fn is_by_name(&self) -> bool {
        self.name.is_some() && self.kind.is_none()
    }

    /// # Errors
    ///
    /// When a `kind` comes without a `name`, a `uid` without a `kind`, or any
    /// given part is empty.
    pub fn check(&self) -> anyhow::Result<()> {
        let empty = |part: &Option<String>| part.as_deref().is_some_and(str::is_empty);
        if self.namespace.is_empty() || empty(&self.name) || empty(&self.kind) || empty(&self.uid) {
            anyhow::bail!("a workload selector's namespace, name, kind and uid cannot be empty");
        }
        if self.kind.is_some() && self.name.is_none() {
            anyhow::bail!("a workload selector with a `kind` needs the owner's `name` too");
        }
        if self.uid.is_some() && self.kind.is_none() {
            anyhow::bail!("a workload selector with a `uid` needs the owner's `kind` and `name`");
        }
        Ok(())
    }

    pub(super) fn admits(&self, workload: &RequestingWorkload) -> bool {
        if self.namespace != workload.namespace {
            return false;
        }
        match (&self.kind, &self.name) {
            (Some(kind), Some(name)) => workload.owner.as_ref().is_some_and(|owner| {
                owner.kind() == kind
                    && owner.name() == name
                    && self.uid.as_ref().is_none_or(|uid| uid == owner.uid())
            }),
            (None, Some(name)) => *name == workload.name,
            (None, None) => true,
            (Some(_), None) => false,
        }
    }
}

impl std::fmt::Display for WorkloadSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.kind, &self.name) {
            (Some(kind), Some(name)) => match &self.uid {
                Some(uid) => write!(f, "`{}/{kind}/{name}` (uid `{uid}`)", self.namespace),
                None => write!(f, "`{}/{kind}/{name}`", self.namespace),
            },
            (None, Some(name)) => write!(f, "`{}/{name}`", self.namespace),
            (Some(kind), None) => {
                write!(f, "`{}/{kind}/` (no name, matches nothing)", self.namespace)
            }
            (None, None) => write!(f, "namespace `{}`", self.namespace),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_selector_shows_its_uid() {
        let owner = WorkloadOwner::new("WorkloadDeployment", "billing", "uid-1").unwrap();
        assert_eq!(
            WorkloadSelector::owner("acme", &owner).to_string(),
            "`acme/WorkloadDeployment/billing` (uid `uid-1`)"
        );
        assert_eq!(
            WorkloadSelector::workload("acme", "billing").to_string(),
            "`acme/billing`"
        );
        let nameless = WorkloadSelector {
            kind: Some("WorkloadDeployment".into()),
            ..WorkloadSelector::namespace("acme")
        };
        assert!(
            nameless.to_string().contains("matches nothing"),
            "{nameless}"
        );
    }

    fn requesting(name: &str, owner: Option<WorkloadOwner>) -> RequestingWorkload {
        RequestingWorkload {
            id: "w1".into(),
            name: name.into(),
            namespace: "billing".into(),
            owner,
        }
    }

    fn deployment(uid: &str) -> WorkloadOwner {
        WorkloadOwner::new("WorkloadDeployment", "api", uid).unwrap()
    }

    #[test]
    fn an_owner_selector_matches_only_that_owner() {
        let selector = WorkloadSelector::owner("billing", &deployment("uid-1"));
        assert!(selector.admits(&requesting("api", Some(deployment("uid-1")))));
        for (name, owner) in [
            ("api", None),
            ("api", Some(deployment("uid-2"))),
            (
                "api",
                Some(WorkloadOwner::new("Workload", "api", "uid-1").unwrap()),
            ),
            (
                "api",
                Some(WorkloadOwner::new("WorkloadDaemonSet", "api", "uid-1").unwrap()),
            ),
        ] {
            assert!(
                !selector.admits(&requesting(name, owner.clone())),
                "{owner:?} took the deployment's grant"
            );
        }
        let mut any_uid = selector.clone();
        any_uid.uid = None;
        assert!(any_uid.admits(&requesting("other", Some(deployment("uid-2")))));
        assert!(!any_uid.admits(&RequestingWorkload {
            namespace: "elsewhere".into(),
            ..requesting("api", Some(deployment("uid-1")))
        }));
    }

    #[test]
    fn a_name_selector_still_matches_the_kept_name() {
        let selector = WorkloadSelector::workload("billing", "api");
        assert!(selector.is_by_name());
        assert!(selector.admits(&requesting("api", None)));
        assert!(!selector.admits(&requesting("other", Some(deployment("uid-1")))));
        assert!(!WorkloadSelector::owner("billing", &deployment("u")).is_by_name());
        assert!(!WorkloadSelector::namespace("billing").is_by_name());
    }

    #[test]
    fn an_incomplete_selector_is_refused() {
        for json in [
            r#"{"namespace":"billing","kind":"WorkloadDeployment"}"#,
            r#"{"namespace":"billing","name":"api","uid":"u"}"#,
            r#"{"namespace":"billing","name":"api","kind":""}"#,
            r#"{"namespace":"","name":"api"}"#,
            r#"{"namespace":"billing","name":"api","owner":"x"}"#,
        ] {
            assert!(
                serde_json::from_str::<WorkloadSelector>(json).is_err(),
                "{json} parsed"
            );
        }
        let parsed: WorkloadSelector = serde_json::from_str(
            r#"{"namespace":"billing","kind":"WorkloadDeployment","name":"api","uid":"u"}"#,
        )
        .unwrap();
        assert_eq!(parsed, WorkloadSelector::owner("billing", &deployment("u")));
        let round = serde_json::to_string(&parsed).unwrap();
        assert_eq!(
            serde_json::from_str::<WorkloadSelector>(&round).unwrap(),
            parsed
        );
    }
}
