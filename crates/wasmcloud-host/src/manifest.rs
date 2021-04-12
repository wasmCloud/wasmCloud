use crate::host_controller::{StartActor, StartProvider};
use crate::messagebus::AdvertiseLink;
use crate::oci::fetch_oci_bytes;
use crate::NativeCapability;
use provider_archive::ProviderArchive;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::{fs::File, io::Read, path::Path};

/// A host manifest contains a declarative profile of the host's desired state. The manifest
/// can specify custom labels, a list of actors, a list of capability providers, and a list of
/// link definitions. Environment substitution syntax can optionally be used within a manifest file so that
/// information that may change across environments (like public keys) can change without requiring
/// the manifest file to change.
///
/// # Examples
///
/// ```yaml
/// labels:
///     sample: "wasmcloud echo"
/// actors:
///     - "wasmcloud.azurecr.io/echo:0.2.0"
/// capabilities:
///     - image_ref: wasmcloud.azurecr.io/httpserver:0.11.1
///       link_name: default
/// links:
///     - actor: ${ECHO_ACTOR:MBCFOPM6JW2APJLXJD3Z5O4CN7CPYJ2B4FTKLJUR5YR5MITIU7HD3WD5}
///       provider_id: "VAG3QITQQ2ODAOWB5TTQSDJ53XK3SHBEIFNK4AYJ5RKAX2UNSCAPHA5M"
///       contract_id: "wasmcloud:httpserver"
///       link_name: default
///       values:
///         PORT: 8080
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostManifest {
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    #[doc(hidden)]
    pub labels: HashMap<String, String>,
    #[doc(hidden)]
    pub actors: Vec<String>,
    #[doc(hidden)]
    pub capabilities: Vec<Capability>,
    #[doc(hidden)]
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<Capability>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkEntry>,
}

/// The description of a capability within a host manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
#[doc(hidden)]
pub struct Capability {
    /// An image reference for this capability. If this is a file on disk, it will be used, otherwise
    /// the system will assume it is an OCI registry image reference
    pub image_ref: String,
    /// The (optional) name of the link that identifies this instance of the capability
    pub link_name: Option<String>,
}

/// A link definition describing the actor and capability provider involved, as well
/// as the configuration values for that link
#[derive(Debug, Clone, Serialize, Deserialize)]
#[doc(hidden)]
pub struct LinkEntry {
    pub actor: String,
    pub contract_id: String,
    pub provider_id: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_name: Option<String>,
    pub values: Option<HashMap<String, String>>,
}

impl HostManifest {
    /// Creates an instance of a host manifest from a file path. The de-serialization
    /// type will be chosen based on the file path extension, selecting YAML for `.yaml`
    /// or `.yml` files, and JSON for all other file extensions. If the path has no extension, the
    /// de-serialization type chosen will be YAML. If `expand_env` is `true` then environment substitution
    /// syntax will be honored in the manifest file.
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
    allowed_insecure: &[String],
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
            if let Ok(a) = fetch_oci_bytes(&actor_ref, allow_latest, allowed_insecure)
                .await
                .and_then(|bytes| crate::Actor::from_slice(&bytes))
            {
                v.push(StartActor {
                    image_ref: Some(actor_ref.to_string()),
                    actor: a,
                });
            } else {
                error!("Actor {} not found on disk or in registry", actor_ref);
            }
        }
    }
    v
}

pub(crate) async fn generate_provider_start_messages(
    manifest: &HostManifest,
    allow_latest: bool,
    allowed_insecure: &[String],
) -> Vec<StartProvider> {
    let mut v = Vec::new();
    for cap in &manifest.capabilities {
        let p = Path::new(&cap.image_ref);
        if p.exists() {
            // read PAR from disk
            if let Ok(prov) = file_bytes(&p)
                .and_then(|bytes| ProviderArchive::try_load(&bytes))
                .and_then(|par| NativeCapability::from_archive(&par, cap.link_name.clone()))
            {
                if let Err(e) = crate::capability::native_host::write_provider_to_disk(&prov) {
                    error!("Could not cache provider to disk: {}", e);
                }
                v.push(StartProvider {
                    provider: prov,
                    image_ref: None,
                })
            }
        } else {
            // read PAR from OCI
            if let Ok(prov) = fetch_oci_bytes(&cap.image_ref, allow_latest, allowed_insecure)
                .await
                .and_then(|bytes| ProviderArchive::try_load(&bytes))
                .and_then(|par| NativeCapability::from_archive(&par, cap.link_name.clone()))
            {
                v.push(StartProvider {
                    provider: prov,
                    image_ref: Some(cap.image_ref.to_string()),
                })
            } else {
                error!(
                    "Provider {} not found on disk or in registry",
                    cap.image_ref
                );
            }
        }
    }

    v
}

pub(crate) async fn generate_adv_link_messages(manifest: &HostManifest) -> Vec<AdvertiseLink> {
    manifest
        .links
        .iter()
        .map(|config| AdvertiseLink {
            contract_id: config.contract_id.to_string(),
            actor: config.actor.to_string(),
            link_name: config
                .link_name
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
    trace!("read {} bytes from file {}", bytes.len(), path.display());
    Ok(bytes)
}

#[cfg(test)]
mod test {
    use super::{Capability, LinkEntry};
    use std::collections::HashMap;

    #[test]
    fn round_trip() {
        let manifest = super::HostManifest {
            labels: HashMap::new(),
            actors: vec!["a".to_string(), "b".to_string(), "c".to_string()],
            capabilities: vec![
                Capability {
                    image_ref: "one".to_string(),
                    link_name: Some("default".to_string()),
                },
                Capability {
                    image_ref: "two".to_string(),
                    link_name: Some("default".to_string()),
                },
            ],
            links: vec![LinkEntry {
                actor: "a".to_string(),
                contract_id: "wasmcloud:one".to_string(),
                provider_id: "Vxxxone".to_string(),
                values: Some(gen_values()),
                link_name: None,
            }],
        };
        let yaml = serde_yaml::to_string(&manifest).unwrap();
        assert_eq!(yaml, "---\nactors:\n  - a\n  - b\n  - c\ncapabilities:\n  - image_ref: one\n    link_name: default\n  - image_ref: two\n    link_name: default\nlinks:\n  - actor: a\n    contract_id: \"wasmcloud:one\"\n    provider_id: Vxxxone\n    values:\n      ROOT: /tmp\n");
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
                    link_name: Some("default".to_string()),
                },
                Capability {
                    image_ref: "two".to_string(),
                    link_name: Some("default".to_string()),
                },
            ],
            links: vec![LinkEntry {
                actor: "a".to_string(),
                contract_id: "wasmcloud:one".to_string(),
                provider_id: "VxxxxONE".to_string(),
                values: Some(gen_values()),
                link_name: Some("default".to_string()),
            }],
        };
        let yaml = serde_yaml::to_string(&manifest).unwrap();
        assert_eq!(yaml, "---\nlabels:\n  test: value\nactors:\n  - a\n  - b\n  - c\ncapabilities:\n  - image_ref: one\n    link_name: default\n  - image_ref: two\n    link_name: default\nlinks:\n  - actor: a\n    contract_id: \"wasmcloud:one\"\n    provider_id: VxxxxONE\n    link_name: default\n    values:\n      ROOT: /tmp\n");
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
