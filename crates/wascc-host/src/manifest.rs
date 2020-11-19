use crate::host_controller::{StartActor, StartProvider};
use crate::messagebus::AdvertiseBinding;
use crate::oci::fetch_oci_bytes;
use crate::NativeCapability;
use provider_archive::ProviderArchive;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::{fs::File, io::Read, path::Path};

/// A host manifest contains a descriptive profile of the host's desired state, including
/// a list of actors and capability providers to load as well as any desired link definitions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostManifest {
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    pub actors: Vec<String>,
    pub capabilities: Vec<Capability>,
    pub bindings: Vec<BindingEntry>,
}

/// The description of a capability within a host manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    /// An image reference for this capability. If this is a file on disk, it will be used, otherwise
    /// the system will assume it is an OCI registry image reference
    pub image_ref: String,
    /// The (optional) name of the link that identifies this instance of the capability
    pub binding_name: Option<String>,
}

/// A link definition describing the actor and capability provider involved, as well
/// as the configuration values for that link
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingEntry {
    pub actor: String,
    pub contract_id: String,
    pub provider_id: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding_name: Option<String>,
    pub values: Option<HashMap<String, String>>,
}

impl HostManifest {
    /// Creates an instance of a host manifest from a file path. The de-serialization
    /// type will be chosen based on the file path extension, selecting YAML for .yaml
    /// or .yml files, and JSON for all other file extensions. If the path has no extension, the
    /// de-serialization type chosen will be YAML.
    pub fn from_path(
        path: impl AsRef<Path>,
        expand_env: bool,
    ) -> std::result::Result<HostManifest, Box<dyn std::error::Error + Send + Sync>> {
        let mut contents = String::new();
        let mut file = File::open(path.as_ref())?;
        file.read_to_string(&mut contents)?;
        if expand_env {
            contents = Self::expand_env(&contents);
        }
        match path.as_ref().extension() {
            Some(e) => {
                let e = e.to_str().unwrap().to_lowercase(); // convert away from the FFI str
                if e == "yaml" || e == "yml" {
                    serde_yaml::from_str::<HostManifest>(&contents).map_err(|e| e.into())
                } else {
                    serde_json::from_str::<HostManifest>(&contents).map_err(|e| e.into())
                }
            }
            None => serde_yaml::from_str::<HostManifest>(&contents).map_err(|e| e.into()),
        }
    }

    fn expand_env(contents: &str) -> String {
        let mut options = envmnt::ExpandOptions::new();
        options.default_to_empty = false; // If environment variable not found, leave unexpanded.
        options.expansion_type = Some(envmnt::ExpansionType::UnixBracketsWithDefaults); // ${VAR:DEFAULT}

        envmnt::expand(contents, Some(options))
    }
}

pub(crate) async fn generate_actor_start_messages(
    manifest: &HostManifest,
    allow_latest: bool,
) -> Vec<StartActor> {
    let mut v = Vec::new();
    for actor_ref in &manifest.actors {
        let p = Path::new(&actor_ref);
        if p.exists() {
            // read actor from disk
            if let Ok(a) = crate::Actor::from_file(p) {
                v.push(StartActor {
                    image_ref: None,
                    actor: a,
                });
            }
        } else {
            // load actor from OCI
            if let Ok(a) = fetch_oci_bytes(&actor_ref, allow_latest)
                .await
                .and_then(|bytes| crate::Actor::from_slice(&bytes))
            {
                v.push(StartActor {
                    image_ref: Some(actor_ref.to_string()),
                    actor: a,
                });
            }
        }
    }
    v
}

pub(crate) async fn generate_provider_start_messages(
    manifest: &HostManifest,
    allow_latest: bool,
) -> Vec<StartProvider> {
    use std::io::Read;

    let mut v = Vec::new();
    for cap in &manifest.capabilities {
        let p = Path::new(&cap.image_ref);
        if p.exists() {
            // read PAR from disk
            if let Ok(prov) = file_bytes(&p)
                .and_then(|bytes| ProviderArchive::try_load(&bytes))
                .and_then(|par| NativeCapability::from_archive(&par, cap.binding_name.clone()))
            {
                v.push(StartProvider {
                    provider: prov,
                    image_ref: None,
                })
            }
        } else {
            // read PAR from OCI
            if let Ok(prov) = fetch_oci_bytes(&cap.image_ref, allow_latest)
                .await
                .and_then(|bytes| ProviderArchive::try_load(&bytes))
                .and_then(|par| NativeCapability::from_archive(&par, cap.binding_name.clone()))
            {
                v.push(StartProvider {
                    provider: prov,
                    image_ref: Some(cap.image_ref.to_string()),
                })
            }
        }
    }

    v
}

pub(crate) async fn generate_adv_binding_messages(
    manifest: &HostManifest,
) -> Vec<AdvertiseBinding> {
    manifest
        .bindings
        .iter()
        .map(|config| AdvertiseBinding {
            contract_id: config.contract_id.to_string(),
            actor: config.actor.to_string(),
            binding_name: config
                .binding_name
                .as_ref()
                .unwrap_or(&"default".to_string())
                .to_string(),
            provider_id: config.provider_id.to_string(),
            values: config.values.as_ref().unwrap_or(&HashMap::new()).clone(),
        })
        .collect()
}

fn file_bytes(path: &Path) -> crate::Result<Vec<u8>> {
    let mut f = File::open(path)?;
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod test {
    use super::{BindingEntry, Capability};
    use std::collections::HashMap;

    #[test]
    fn round_trip() {
        let manifest = super::HostManifest {
            labels: HashMap::new(),
            actors: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            capabilities: vec![
                Capability {
                    image_ref: "one".to_string(),
                    binding_name: Some("default".to_string()),
                },
                Capability {
                    image_ref: "two".to_string(),
                    binding_name: Some("default".to_string()),
                },
            ],
            bindings: vec![BindingEntry {
                actor: "a".to_string(),
                contract_id: "wascc:one".to_string(),
                provider_id: "Vxxxone".to_string(),
                values: Some(gen_values()),
                binding_name: None,
            }],
        };
        let yaml = serde_yaml::to_string(&manifest).unwrap();
        assert_eq!(yaml, "---\nactors:\n  - a\n  - b\n  - c\ncapabilities:\n  - image_ref: one\n    binding_name: default\n  - image_ref: two\n    binding_name: default\nbindings:\n  - actor: a\n    contract_id: \"wascc:one\"\n    provider_id: Vxxxone\n    values:\n      ROOT: /tmp");
    }

    #[test]
    fn round_trip_with_labels() {
        let manifest = super::HostManifest {
            labels: {
                let mut hm = HashMap::new();
                hm.insert("test".to_string(), "value".to_string());
                hm
            },
            actors: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            capabilities: vec![
                Capability {
                    image_ref: "one".to_string(),
                    binding_name: Some("default".to_string()),
                },
                Capability {
                    image_ref: "two".to_string(),
                    binding_name: Some("default".to_string()),
                },
            ],
            bindings: vec![BindingEntry {
                actor: "a".to_string(),
                contract_id: "wascc:one".to_string(),
                provider_id: "VxxxxONE".to_string(),
                values: Some(gen_values()),
                binding_name: Some("default".to_string()),
            }],
        };
        let yaml = serde_yaml::to_string(&manifest).unwrap();
        assert_eq!(yaml, "---\nlabels:\n  test: value\nactors:\n  - a\n  - b\n  - c\ncapabilities:\n  - image_ref: one\n    binding_name: default\n  - image_ref: two\n    binding_name: default\nbindings:\n  - actor: a\n    contract_id: \"wascc:one\"\n    provider_id: VxxxxONE\n    binding_name: default\n    values:\n      ROOT: /tmp");
    }

    #[test]
    fn env_expansion() {
        let values = vec![
            "echo Test",
            "echo $TEST_EXPAND_ENV_TEMP",
            "echo ${TEST_EXPAND_ENV_TEMP}",
            "echo ${TEST_EXPAND_ENV_TMP}",
            "echo ${TEST_EXPAND_ENV_TEMP:/etc}",
            "echo ${TEST_EXPAND_ENV_TMP:/etc}",
        ];
        let expected = vec![
            "echo Test",
            "echo $TEST_EXPAND_ENV_TEMP",
            "echo /tmp",
            "echo ${TEST_EXPAND_ENV_TMP}",
            "echo /tmp",
            "echo /etc",
        ];

        envmnt::set("TEST_EXPAND_ENV_TEMP", "/tmp");
        for (got, expected) in values
            .iter()
            .map(|v| super::HostManifest::expand_env(v))
            .zip(expected.iter())
        {
            assert_eq!(*expected, got);
        }
        envmnt::remove("TEST_EXPAND_ENV_TEMP");
    }

    fn gen_values() -> HashMap<String, String> {
        let mut hm = HashMap::new();
        hm.insert("ROOT".to_string(), "/tmp".to_string());

        hm
    }
}
