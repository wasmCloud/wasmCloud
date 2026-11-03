//! Workload grants for plugin-backed secrets.

use super::{RequestingWorkload, WorkloadSelector};

/// Workloads allowed to use a plugin-backed `secrets:` entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WorkloadGrant {
    /// Every workload on the host: `workloads: all`.
    All,
    /// Workloads matching one of these, never empty.
    Only(Vec<WorkloadSelector>),
}

impl WorkloadGrant {
    /// Whether `workload` may bind a binding that reads the entry.
    pub fn admits(&self, workload: &RequestingWorkload) -> bool {
        match self {
            Self::All => true,
            Self::Only(selectors) => selectors.iter().any(|s| s.admits(workload)),
        }
    }
}

impl std::fmt::Display for WorkloadGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => f.write_str("every workload"),
            Self::Only(selectors) => {
                let names: Vec<String> = selectors
                    .iter()
                    .map(|s| match &s.name {
                        Some(name) => format!("`{}/{name}`", s.namespace),
                        None => format!("namespace `{}`", s.namespace),
                    })
                    .collect();
                f.write_str(&names.join(", "))
            }
        }
    }
}

/// Serialized as `all` or a nonempty list of selectors.
impl serde::Serialize for WorkloadGrant {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::All => s.serialize_str("all"),
            Self::Only(selectors) if selectors.is_empty() => Err(serde::ser::Error::custom(
                "`workloads` grants no workload; name at least one namespace, or write `all`",
            )),
            Self::Only(selectors) => selectors.serialize(s),
        }
    }
}

impl<'de> serde::Deserialize<'de> for WorkloadGrant {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Grant;

        impl<'de> serde::de::Visitor<'de> for Grant {
            type Value = WorkloadGrant;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("`all`, or a list of `namespace` (and optional `name`) selectors")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<WorkloadGrant, E> {
                match v {
                    "all" => Ok(WorkloadGrant::All),
                    other => Err(E::invalid_value(serde::de::Unexpected::Str(other), &self)),
                }
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<WorkloadGrant, A::Error> {
                let mut selectors = Vec::new();
                while let Some(selector) = seq.next_element()? {
                    selectors.push(selector);
                }
                if selectors.is_empty() {
                    return Err(serde::de::Error::custom(
                        "`workloads` grants no workload; name at least one namespace, or write \
                         `all`",
                    ));
                }
                Ok(WorkloadGrant::Only(selectors))
            }
        }

        d.deserialize_any(Grant)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_round_trips() {
        for grant in [
            WorkloadGrant::All,
            WorkloadGrant::Only(vec![WorkloadSelector::workload("billing", "billing-api")]),
        ] {
            let json = serde_json::to_string(&grant).unwrap();
            assert_eq!(serde_json::from_str::<WorkloadGrant>(&json).unwrap(), grant);
        }
        let err = serde_json::to_string(&WorkloadGrant::Only(Vec::new())).unwrap_err();
        assert!(err.to_string().contains("grants no workload"), "{err}");
    }
}
