use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs::File, io::Read, path::Path};

/// A host manifest contains a declarative profile of the host's desired state. The manifest
/// can specify a list of actors, a list of capability providers, and a list of
/// link definitions. Environment substitution syntax can optionally be used within a manifest file so that
/// information that may change across environments (like public keys) can change without requiring
/// the manifest file to change.
///
/// # Examples
///
/// ```yaml
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
    #[doc(hidden)]
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<String>,
    #[doc(hidden)]
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<Capability>,
    #[doc(hidden)]
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
