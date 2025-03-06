//! Parse wasmcloud.toml files which specify key information for building and signing
//! WebAssembly modules and native capability provider binaries

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{anyhow, bail, Context, Result};
use cargo_toml::{Manifest, Product};
use config::Config;
use semver::{Version, VersionReq};
use serde::{Deserialize, Deserializer};
use tracing::{trace, warn};
use url::Url;
use wadm_types::{Component, Properties, SecretSourceProperty};
use wasm_pkg_client::{CustomConfig, Registry, RegistryMapping, RegistryMetadata};
use wasm_pkg_core::config::{Config as PackageConfig, Override};
use wasmcloud_control_interface::RegistryCredential;
use wasmcloud_core::{parse_wit_package_name, WitFunction, WitInterface, WitNamespace, WitPackage};

#[derive(Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum LanguageConfig {
    Rust(RustConfig),
    TinyGo(TinyGoConfig),
    Go(GoConfig),
    Other(String),
}

#[allow(clippy::large_enum_variant)]
#[derive(Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TypeConfig {
    #[serde(alias = "component")]
    Component(ComponentConfig),
    Provider(ProviderConfig),
}

impl TypeConfig {
    #[must_use]
    pub const fn wit_world(&self) -> &Option<String> {
        match self {
            Self::Component(c) => &c.wit_world,
            Self::Provider(c) => &c.wit_world,
        }
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct ComponentConfig {
    /// The directory to store the private signing keys in.
    #[serde(default = "default_key_directory")]
    pub key_directory: PathBuf,
    /// The target wasm target to build for. Defaults to "wasm32-unknown-unknown" (a WASM core module).
    #[serde(default, deserialize_with = "wasm_target")]
    pub wasm_target: WasmTarget,
    /// Path to a wasm adapter that can be used for wasip2
    pub wasip1_adapter_path: Option<PathBuf>,
    /// The WIT world that is implemented by the component
    pub wit_world: Option<String>,
    /// Tags that should be applied during the component signing process
    pub tags: Option<HashSet<String>>,
    /// File path `wash` can use to find the built artifact. Defaults to `./build/[name].wasm`
    pub build_artifact: Option<PathBuf>,
    /// Optional build override command to run instead of attempting to use the native language
    /// toolchain to build. Keep in mind that `wash` expects for the built artifact to be located
    /// under the `build` directory of the project root unless overridden by `build_artifact`.
    pub build_command: Option<String>,
    /// File path the built and signed component should be written to. Defaults to `./build/[name]_s.wasm`
    pub destination: Option<PathBuf>,
}

/// Custom deserializer to parse the wasm target string into a [`WasmTarget`] enum
fn wasm_target<'de, D>(target: D) -> Result<WasmTarget, D::Error>
where
    D: Deserializer<'de>,
{
    let target = String::deserialize(target)?;
    Ok(target.as_str().into())
}

impl RustConfig {
    #[must_use]
    pub const fn build_target(&self, wasm_target: &WasmTarget) -> &'static str {
        match wasm_target {
            WasmTarget::CoreModule => "wasm32-unknown-unknown",
            WasmTarget::WasiP1 => "wasm32-wasip1",
            WasmTarget::WasiP2 => "wasm32-wasip2",
        }
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct ProviderConfig {
    /// The vendor name of the provider.
    #[serde(default = "default_vendor")]
    pub vendor: String,
    /// Optional WIT world for the provider, e.g. `wasmcloud:messaging`
    pub wit_world: Option<String>,
    /// The target operating system of the provider archive. Defaults to the current OS.
    #[serde(default = "default_os")]
    pub os: String,
    /// The target architecture of the provider archive. Defaults to the current architecture.
    #[serde(default = "default_arch")]
    pub arch: String,
    /// The Rust target triple to build for. Defaults to the default rust toolchain.
    pub rust_target: Option<String>,
    /// Optional override for the provider binary name, required if we cannot infer this from Cargo.toml
    pub bin_name: Option<String>,
    /// The directory to store the private signing keys in.
    #[serde(default = "default_key_directory")]
    pub key_directory: PathBuf,
}

fn default_vendor() -> String {
    "NoVendor".to_string()
}
fn default_os() -> String {
    std::env::consts::OS.to_string()
}
fn default_arch() -> String {
    std::env::consts::ARCH.to_string()
}
fn default_key_directory() -> PathBuf {
    let home_dir = etcetera::home_dir().unwrap();
    home_dir.join(".wash/keys")
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct RustConfig {
    /// The path to the cargo binary. Optional, will default to search the user's `PATH` for `cargo` if not specified.
    pub cargo_path: Option<PathBuf>,
    /// Path to cargo/rust's `target` directory. Optional, defaults to the cargo target directory for the workspace or project.
    pub target_path: Option<PathBuf>,
    // Whether to build in debug mode. Defaults to false.
    #[serde(default)]
    pub debug: bool,
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct RegistryConfig {
    /// Configuration to use when pushing this project to a registry
    // NOTE: flattened for backwards compatibility
    #[serde(flatten)]
    pub push: RegistryPushConfig,

    /// Configuration to use for pulling from registries
    pub pull: Option<RegistryPullConfig>,
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct RegistryPushConfig {
    /// URL of the registry to push to
    pub url: Option<String>,

    /// Credentials to use for the given registry
    pub credentials: Option<PathBuf>,

    /// Whether or not to push to the registry insecurely with http
    #[serde(default)]
    pub push_insecure: bool,
}

/// Configuration that governs pulling of packages from registries
#[derive(Debug, Default, PartialEq, Eq, Clone, Deserialize)]
pub struct RegistryPullConfig {
    /// List of sources that should be pulled
    pub sources: Vec<RegistryPullSourceOverride>,
}

/// Information identifying a registry that can be pulled from
#[derive(Debug, Default, PartialEq, Eq, Clone, Deserialize)]
pub struct RegistryPullSourceOverride {
    /// Target specification for which this source applies (usually a namespace and/or package)
    ///
    /// ex. `wasi`, `wasi:keyvalue`, `wasi:keyvalue@0.2.0`
    pub target: String,

    /// The source for the configuration
    pub source: RegistryPullSource,
}

/// Source for a registry pull
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub enum RegistryPullSource {
    /// Sources for interfaces that are built-in/resolvable without configuration,
    /// or special-cased in some other way
    /// (e.g. `wasi:http` is a well known standard)
    #[default]
    Builtin,

    /// A file source
    ///
    /// These references are resolved in two ways:
    ///   - If a directory, then the namespace & path are appended
    ///   - If a direct file then the file itself is used
    ///
    /// (ex. '<file://relative/path/to/file>', '<file:///absolute/path/to/file>')
    LocalPath(String),

    /// Remote HTTP registry, configured to support `.well-known/wasm-pkg/registry.json`
    RemoteHttpWellKnown(String),

    /// An OCI reference
    ///
    /// These references are resolved by appending the intended namespace and package
    /// to the provided URI
    ///
    /// (ex. resolving `wasi:keyvalue@0.2.0` with '<oci://ghcr.io/wasmcloud/wit>' becomes `oci://ghcr.io/wasmcloud/wit/wasi/keyvalue:0.2.0`)
    RemoteOci(String),

    /// URL to a HTTP/S resource
    ///
    /// These references are resolved by downloading and uncompressing (where possible) the linked file
    /// as WIT, for whatever interfaces were provided.
    ///
    /// (ex. resolving `https://example.com/wit/package.tgz` means downloading and unpacking the tarball)
    RemoteHttp(String),

    /// URL to a GIT repository
    ///
    /// These URLs are guaranteed to start with a git-related scheme (ex. `git+http://`, `git+ssh://`, ...)
    /// and will be used as the base under which to pull a folder of WIT
    RemoteGit(String),
}

impl Display for RegistryPullSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Builtin => write!(f, "builtin")?,
            Self::LocalPath(s)
            | Self::RemoteHttpWellKnown(s)
            | Self::RemoteOci(s)
            | Self::RemoteHttp(s)
            | Self::RemoteGit(s) => write!(f, "{s}")?,
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for RegistryPullSource {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl TryFrom<String> for RegistryPullSource {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self> {
        Self::from_str(&value)
    }
}

impl FromStr for RegistryPullSource {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s {
            s if s.starts_with("file://") => Self::LocalPath(s.into()),
            s if s.starts_with("oci://") => Self::RemoteOci(s.into()),
            s if s.starts_with("http://") || s.starts_with("https://") => {
                Self::RemoteHttp(s.into())
            }
            s if s.starts_with("git+ssh://")
                || s.starts_with("git+http://")
                || s.starts_with("git+https://") =>
            {
                Self::RemoteGit(s.into())
            }
            "builtin" => Self::Builtin,
            s => bail!("unrecognized registry pull source [{s}]"),
        })
    }
}

impl RegistryPullSource {
    pub async fn resolve_file_path(&self, base_dir: impl AsRef<Path>) -> Result<PathBuf> {
        match self {
            Self::LocalPath(p) => match p.strip_prefix("file://") {
                Some(s) if s.starts_with('/') => tokio::fs::canonicalize(s)
                    .await
                    .with_context(|| format!("failed to canonicalize absolute path [{s}]")),
                Some(s) => tokio::fs::canonicalize(base_dir.as_ref().join(s))
                    .await
                    .with_context(|| format!("failed to canonicalize relative path [{s}]")),
                None => bail!("invalid RegistryPullSource file path [{p}]"),
            },
            _ => bail!("registry pull source does not resolve to file path"),
        }
    }
}

impl TryFrom<RegistryPullSource> for RegistryMapping {
    type Error = anyhow::Error;

    fn try_from(value: RegistryPullSource) -> Result<Self> {
        match value {
            RegistryPullSource::Builtin | RegistryPullSource::LocalPath(_) => {
                bail!("builtins and local files cannot be converted to registry mappings")
            }
            RegistryPullSource::RemoteHttp(_) => {
                bail!("remote files HTTP files cannot be converted to registry mappings")
            }
            RegistryPullSource::RemoteGit(_) => {
                bail!("remote git repositories files cannot be converted to registry mappings")
            }
            // For well known strings, we generally expect to receive a HTTP/S URL
            RegistryPullSource::RemoteHttpWellKnown(url) => {
                let url = Url::parse(&url).context("failed to parse url")?;
                Registry::from_str(url.as_str())
                    .map(RegistryMapping::Registry)
                    .map_err(|e| anyhow!(e))
            }
            // For remote OCI images we expect to receive an 'oci://' prefixed String which we treat as a URI
            //
            // ex. `oci://ghcr.io/wasmcloud/interfaces` will turn into a registry with:
            // - `ghcr.io` as the base
            // - `wasmcloud/interfaces` as the namespace prefix
            //
            RegistryPullSource::RemoteOci(uri) => {
                let url = Url::parse(&uri).context("failed to parse url")?;
                if url.scheme() != "oci" {
                    bail!("invalid scheme [{}], expected 'oci'", url.scheme());
                }
                let metadata = {
                    let mut metadata = RegistryMetadata::default();
                    metadata.preferred_protocol = Some("oci".into());
                    let mut protocol_configs = serde_json::Map::new();
                    let namespace_prefix = format!(
                        "{}/",
                        url.path().strip_prefix('/').unwrap_or_else(|| url.path())
                    );
                    protocol_configs.insert(
                        "namespacePrefix".into(),
                        serde_json::json!(namespace_prefix),
                    );
                    metadata.protocol_configs = HashMap::from([("oci".into(), protocol_configs)]);
                    metadata
                };
                Ok(Self::Custom(CustomConfig {
                    registry: Registry::from_str(&format!(
                        "{}{}",
                        url.authority(),
                        url.port().map(|p| format!(":{p}")).unwrap_or_default()
                    ))
                    .map_err(|e| anyhow!(e))?,
                    metadata,
                }))
            }
        }
    }
}

/// Configuration common among all project types & languages.
#[derive(Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct CommonConfig {
    /// Name of the project.
    pub name: String,
    /// Semantic version of the project.
    pub version: Version,
    /// Monotonically increasing revision number
    pub revision: i32,
    /// Path to the project root to determine where build commands should be run.
    pub project_dir: PathBuf,
    /// Path to the directory where built artifacts should be written. Defaults to a `build` directory
    /// in the project root.
    pub build_dir: PathBuf,
    /// Path to the directory where the WIT world and dependencies can be found. Defaults to a `wit`
    /// directory in the project root.
    pub wit_dir: PathBuf,
    /// Expected name of the wasm module binary that will be generated
    /// (if not present, name is expected to be used as a fallback)
    pub wasm_bin_name: Option<String>,
    /// Optional artifact OCI registry configuration. Primarily used for `wash push` & `wash pull` commands
    pub registry: RegistryConfig,
}

impl CommonConfig {
    /// Helper function to get the Wasm name, falling back to the project name if not specified
    #[must_use]
    pub fn wasm_bin_name(&self) -> String {
        self.wasm_bin_name
            .clone()
            .unwrap_or_else(|| self.name.clone())
    }
}

#[derive(Debug, Deserialize, Default, Clone, Eq, PartialEq)]
pub enum WasmTarget {
    #[default]
    #[serde(alias = "wasm32-unknown-unknown")]
    CoreModule,
    #[serde(
        alias = "wasm32-wasi",
        alias = "wasm32-wasi-preview1",
        alias = "wasm32-wasip1"
    )]
    WasiP1,
    #[serde(
        alias = "wasm32-wasip2",
        alias = "wasm32-wasi-preview2",
        alias = "wasm32-preview2"
    )]
    WasiP2,
}

impl From<&str> for WasmTarget {
    fn from(value: &str) -> Self {
        match value {
            "wasm32-wasi-preview1" => Self::WasiP1,
            "wasm32-wasip1" => Self::WasiP1,
            "wasm32-wasi" => Self::WasiP1,
            "wasm32-wasi-preview2" => Self::WasiP2,
            "wasm32-wasip2" => Self::WasiP2,
            "wasm32-unknown-unknown" => Self::CoreModule,
            _ => {
                warn!("Unknown wasm_target `{value}`, expected wasm32-wasip2 or wasm32-wasip1. Defaulting to wasm32-unknown-unknown");
                Self::CoreModule
            }
        }
    }
}

impl From<String> for WasmTarget {
    fn from(value: String) -> Self {
        value.as_str().into()
    }
}

impl Display for WasmTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match &self {
            Self::CoreModule => "wasm32-unknown-unknown",
            Self::WasiP1 => "wasm32-wasip1",
            Self::WasiP2 => "wasm32-wasip2",
        })
    }
}

/// Configuration related to Golang configuration
#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct GoConfig {
    /// The path to the go binary. Optional, will default to `go` if not specified.
    pub go_path: Option<PathBuf>,
    /// Whether to disable the `go generate` step in the build process. Defaults to false.
    #[serde(default)]
    pub disable_go_generate: bool,
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TinyGoScheduler {
    None,
    Tasks,
    Asyncify,
}

impl TinyGoScheduler {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Tasks => "tasks",
            Self::Asyncify => "asyncify",
        }
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TinyGoGarbageCollector {
    None,
    Conservative,
    Leaking,
}

impl TinyGoGarbageCollector {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Conservative => "conservative",
            Self::Leaking => "leaking",
        }
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct TinyGoConfig {
    /// The path to the tinygo binary. Optional, will default to `tinygo` if not specified.
    pub tinygo_path: Option<PathBuf>,
    /// Whether to disable the `go generate` step in the build process. Defaults to false.
    #[serde(default)]
    pub disable_go_generate: bool,
    /// The scheduler to use for the `TinyGo` build.
    ///
    /// Override the default scheduler (asyncify). Valid values are: none, tasks, asyncify.
    pub scheduler: Option<TinyGoScheduler>,
    /// The garbage collector to use for the `TinyGo` build.
    ///
    /// Override the default garbage collector (conservative). Valid values are: none, conservative, leaking.
    pub garbage_collector: Option<TinyGoGarbageCollector>,
}

impl TinyGoConfig {
    #[must_use]
    pub const fn build_target(&self, wasm_target: &WasmTarget) -> &'static str {
        match wasm_target {
            WasmTarget::CoreModule => "wasm",
            WasmTarget::WasiP1 => "wasi",
            WasmTarget::WasiP2 => "wasip2",
        }
    }
}

/// Specification for how to wire up configuration
#[derive(Debug, PartialEq, Eq, Clone, Deserialize)]
#[serde(untagged)]
pub enum DevConfigSpec {
    /// Existing config with the given name
    Named { name: String },
    /// Explicitly specified configuration with all values presented
    Values { values: BTreeMap<String, String> },
}

/// Specification for how to wire up secrets
#[derive(Debug, PartialEq, Eq, Clone, Deserialize)]
#[serde(untagged)]
pub enum DevSecretSpec {
    /// Existing secret with a name and source properties
    Existing {
        name: String,
        source: SecretSourceProperty,
    },
    /// Explicitly specified secret values with all values presented
    ///
    /// NOTE: Secret names are required at all times, since secrets
    /// *must* be named when interacting with a secret store
    Values {
        name: String,
        values: BTreeMap<String, String>,
    },
}

/// Target that specifies a single component in a given manifest path
#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize)]
pub struct DevManifestComponentTarget {
    /// Name of the component that should be targeted
    pub component_name: Option<String>,

    /// The ID of the component that should be targeted
    pub component_id: Option<String>,

    /// The image reference of the component that should be targeted
    pub component_ref: Option<String>,

    /// The manifest in which the target exists
    pub path: PathBuf,
}

impl DevManifestComponentTarget {
    #[must_use]
    pub fn matches(&self, component: &Component) -> bool {
        let (component_id, component_ref) = match &component.properties {
            Properties::Component { ref properties } => (&properties.id, &properties.image),
            Properties::Capability { ref properties } => (&properties.id, &properties.image),
        };

        if self
            .component_name
            .as_ref()
            .is_some_and(|v| v == &component.name)
        {
            return true;
        }

        if self
            .component_id
            .as_ref()
            .is_some_and(|a| component_id.as_ref().is_some_and(|b| a == b))
        {
            return true;
        }

        if self
            .component_ref
            .as_ref()
            .is_some_and(|v| component_ref.as_ref().is_some_and(|c| c == v))
        {
            return true;
        }

        false
    }
}

/// Interface-based overrides used for a single component
#[derive(Default, Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InterfaceComponentOverride {
    /// Specification of the interface
    ///
    /// ex. `wasi:keyvalue@0.2.0`, `wasi:http/incoming-handler@0.2.0`
    #[serde(alias = "interface")]
    pub interface_spec: String,

    /// Configuration that should be provided to the overridden component
    pub config: Option<OneOrMore<DevConfigSpec>>,

    /// Secrets that should be provided to the overridden component
    pub secrets: Option<OneOrMore<DevSecretSpec>>,

    /// Reference to the component
    #[serde(alias = "uri")]
    pub image_ref: Option<String>,

    /// Link name that should be used to reference the component
    ///
    /// This is only required when there are *more than one* overrides that conflict (i.e. there is no "default")
    pub link_name: Option<String>,
}

/// String that represents a specification of a WIT interface (normally used when specifying [`InterfaceComponentOverride`]s)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitInterfaceSpec {
    /// WIT namespace
    pub namespace: WitNamespace,
    /// WIT package name
    pub package: WitPackage,
    /// WIT interfaces, if omitted will be used to match any interface
    pub interfaces: Option<HashSet<WitInterface>>,
    /// WIT interface function
    pub function: Option<WitFunction>,
    /// Version of WIT interface
    pub version: Option<Version>,
}

impl WitInterfaceSpec {
    /// Check whether this wit interface specification "contains" another one
    ///
    /// Containing another WIT interface spec means the current interface (if loosely specified)
    /// is *more* general than the `other` one.
    ///
    /// This means that if the `other` spec is more general, this one will count as overlapping with it.
    ///
    /// ```
    /// use std::str::FromStr;
    /// use crate::lib::parser::WitInterfaceSpec;
    /// assert!(WitInterfaceSpec::from_str("wasi:http").unwrap().includes(WitInterfaceSpec::from_str("wasi:http/incoming-handler").as_ref().unwrap()));
    /// assert!(WitInterfaceSpec::from_str("wasi:http/incoming-handler").unwrap().includes(WitInterfaceSpec::from_str("wasi:http/incoming-handler.handle").as_ref().unwrap()));
    /// ```
    #[must_use]
    pub fn includes(&self, other: &Self) -> bool {
        !self.is_disjoint(other)
    }

    #[must_use]
    pub fn is_disjoint(&self, other: &Self) -> bool {
        if self.namespace != other.namespace {
            return true;
        }
        if self.package != other.package {
            return true;
        }
        // If interfaces don't match, this interface can't contain the other one
        match (self.interfaces.as_ref(), other.interfaces.as_ref()) {
            // If they both have no interface specified, then we do overlap
            (None | Some(_), None) | (None, Some(_)) => {
                return false;
            }
            // If both specify different interfaces, we don't overlap
            (Some(iface), Some(other_iface)) if iface != other_iface => {
                return true;
            }
            // The only option left is when the interfaces are the same
            (Some(_), Some(_)) => {}
        }

        // At this point, we know that the interfaces must match
        match (self.function.as_ref(), other.function.as_ref()) {
            // If neither have functions, they cannot be disjoint
            (None | Some(_), None) | (None, Some(_)) => {
                return false;
            }
            // If the functions differ, these are disjoint
            (Some(f), Some(other_f)) if f != other_f => {
                return true;
            }
            // The only option left is when the functions are the same
            (Some(_), Some(_)) => {}
        }

        // Compare the versions
        match (self.version.as_ref(), other.version.as_ref()) {
            // If the neither have versions, they cannot be disjoint
            (None | Some(_), None) | (None, Some(_)) => false,
            // If the *either* version matches the other in semantic version terms, they cannot be disjoint
            //
            // Realistically this means that 0.2.0 and 0.2.1 are *not* disjoint, and while they could be,
            // we assume that semantic versioning semantics should ensure that 0.2.0 and 0.2.1 are backwards compatible
            // (though for <1.x versions, there is no such "real" guarantee)
            //
            (Some(v), Some(other_v))
                if VersionReq::parse(&format!("^{v}")).is_ok_and(|req| req.matches(other_v)) =>
            {
                false
            }
            (Some(v), Some(other_v))
                if VersionReq::parse(&format!("^{other_v}")).is_ok_and(|req| req.matches(v)) =>
            {
                false
            }
            // The only option left is that the versions are the same and their versions are incompatible/different
            _ => true,
        }
    }
}

impl std::str::FromStr for WitInterfaceSpec {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match parse_wit_package_name(s) {
            Ok((namespace, packages, interfaces, function, version))
                if packages.len() == 1
                    && (interfaces.is_none()
                        || interfaces.as_ref().is_some_and(|v| v.len() == 1)) =>
            {
                Ok(Self {
                    namespace,
                    package: packages
                        .into_iter()
                        .next()
                        .context("unexpectedly missing package")?,
                    interfaces: match interfaces {
                        Some(v) if v.is_empty() => bail!("unexpectedly missing interface"),
                        Some(v) => Some(v.into_iter().collect()),
                        None => None,
                    },
                    function,
                    version,
                })
            }
            Ok((_, _, _, Some(_), _)) => {
                bail!("function-level interface overrides are not yet supported")
            }
            Ok(_) => bail!("nested interfaces not yet supported"),
            Err(e) => bail!("failed to parse WIT interface spec (\"{s}\"): {e}"),
        }
    }
}

impl<'de> Deserialize<'de> for WitInterfaceSpec {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Multi {
            Stringified(String),
            Explicit {
                namespace: String,
                package: String,
                interface: Option<String>,
                function: Option<String>,
                version: Option<Version>,
            },
        }

        match Multi::deserialize(deserializer)? {
            Multi::Stringified(s) => Self::from_str(&s).map_err(|e| {
                serde::de::Error::custom(format!(
                    "failed to parse WIT interface specification: {e}"
                ))
            }),
            Multi::Explicit {
                namespace,
                package,
                interface,
                function,
                version,
            } => Ok(Self {
                namespace,
                package,
                interfaces: interface.map(|i| HashSet::from([i])),
                function,
                version,
            }),
        }
    }
}

/// Facilitates *one* of a given `T` or more (primarily for serde use)
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum OneOrMore<T> {
    /// Only one of the given type
    One(T),
    /// More than one T
    More(Vec<T>),
}

impl<T> OneOrMore<T> {
    /// Convert this `OneOrMore<T>` into a `Vec<T>`
    #[allow(unused)]
    fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(t) => vec![t],
            Self::More(ts) => ts,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        OneOrMoreIterator {
            inner: self,
            idx: 0,
        }
    }
}

/// Iterator for [`OneOrMore`]
pub struct OneOrMoreIterator<'a, T> {
    inner: &'a OneOrMore<T>,
    idx: usize,
}

impl<'a, T> Iterator for OneOrMoreIterator<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        match (self.idx, self.inner) {
            (0, OneOrMore::One(inner)) => {
                if let Some(v) = self.idx.checked_add(1) {
                    self.idx = v;
                }
                Some(inner)
            }
            (_, OneOrMore::One(_)) => None,
            (idx, OneOrMore::More(vs)) => {
                if let Some(v) = self.idx.checked_add(1) {
                    self.idx = v;
                }
                vs.get(idx)
            }
        }
    }
}

/// Configuration for imports that should be overridden
#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize)]
pub struct InterfaceOverrides {
    /// Imports that should be overridden
    #[serde(default)]
    pub imports: Vec<InterfaceComponentOverride>,

    /// Exports that should be overridden
    #[serde(default)]
    pub exports: Vec<InterfaceComponentOverride>,
}

/// Configuration for development environments and/or DX related plugins
#[derive(Default, Debug, PartialEq, Eq, Clone, Deserialize)]
pub struct DevConfig {
    /// Top level override of the WADM application manifest(s) to use for development
    ///
    /// If this value is specified, tooling should strive to use the provided manifest where possible.
    /// If unspecified, it is up to tools to generate a manifest from available information.
    #[serde(default)]
    pub manifests: Vec<DevManifestComponentTarget>,

    /// Configuration values to be passed to the component
    #[serde(default, alias = "configs")]
    pub config: Vec<DevConfigSpec>,

    /// Configuration values to be passed to the component
    #[serde(default)]
    pub secrets: Vec<DevSecretSpec>,

    /// Interface-driven overrides
    ///
    /// Normally keyed by strings that represent an interface specification (e.g. `wasi:keyvalue/store@0.2.0-draft`)
    #[serde(default)]
    pub overrides: InterfaceOverrides,
}

/// Gets the wasmCloud project (component or provider) config.
///
/// The config can come from multiple sources: a specific toml file path, a folder with a `wasmcloud.toml` file inside it, or by default it looks for a `wasmcloud.toml` file in the current directory.
///
/// The user can also override the config file by setting environment variables with the prefix "WASMCLOUD_". This behavior can be disabled by setting `use_env` to false.
/// For example, a user could set the variable `WASMCLOUD_RUST_CARGO_PATH` to override the default `cargo` path.
///
/// # Arguments
/// * `opt_path` - The path to the config file. If None, it will look for a wasmcloud.toml file in the current directory.
/// * `use_env` - Whether to use the environment variables or not. If false, it will not attempt to use environment variables. Defaults to true.
pub async fn load_config(
    opt_path: Option<PathBuf>,
    use_env: Option<bool>,
) -> Result<ProjectConfig> {
    let project_dir = match opt_path.clone() {
        Some(p) => p,
        None => std::env::current_dir().context("failed to get current directory")?,
    };

    let path = if !project_dir.exists() {
        bail!("path {} does not exist", project_dir.display());
    } else {
        fs::canonicalize(&project_dir).context("failed to canonicalize project path")?
    };

    let (wasmcloud_toml_dir, wasmcloud_toml_path) = if path.is_dir() {
        let wasmcloud_path = path.join("wasmcloud.toml");
        if !wasmcloud_path.is_file() {
            bail!("failed to find wasmcloud.toml in [{}]", path.display());
        }
        (path, wasmcloud_path)
    } else if path.is_file() {
        (
            path.parent()
                .ok_or_else(|| anyhow!("Could not get parent path of wasmcloud.toml file"))?
                .to_path_buf(),
            path,
        )
    } else {
        bail!(
            "failed to find wasmcloud.toml: path [{}] is not a directory or file",
            path.display()
        );
    };

    let mut config = Config::builder().add_source(config::File::from(wasmcloud_toml_path.clone()));

    if use_env.unwrap_or(true) {
        config = config.add_source(config::Environment::with_prefix("WASMCLOUD"));
    }

    let json_value = config
        .build()
        .map_err(|e| {
            if e.to_string().contains("is not of a registered file format") {
                return anyhow!("invalid config file: {}", wasmcloud_toml_path.display());
            }

            anyhow!("{}", e)
        })?
        .try_deserialize::<serde_json::Value>()?;

    let mut toml_project_config: WasmcloudDotToml = serde_json::from_value(json_value)?;
    // NOTE(thomastaylor312): Because the package config fields have serde default, they get set,
    // even if nothing was set in the toml file. So, if the config is equal to the default, we set
    // things to None.
    let current_config = toml_project_config
        .package_config
        .take()
        .unwrap_or_default();
    if current_config != PackageConfig::default() {
        toml_project_config.package_config = Some(current_config);
    }
    if toml_project_config.package_config.is_none() {
        // Attempt to load the package config from wkg.toml if it wasn't set in wasmcloud.toml
        let wkg_toml_path = wasmcloud_toml_dir.join(wasm_pkg_core::config::CONFIG_FILE_NAME);
        // If the file exists, we attempt to load it. We don't want to warn if it doesn't exist.
        // If it does exist, we want to warn if it's invalid.
        match tokio::fs::metadata(&wkg_toml_path).await {
            Ok(meta) if meta.is_file() => {
                match PackageConfig::load_from_path(wkg_toml_path).await {
                    Ok(wkg_config) => {
                        toml_project_config.package_config = Some(wkg_config);
                    }
                    Err(e) => {
                        tracing::warn!(err = %e, "failed to load wkg.toml");
                    }
                }
            }
            Ok(_) => (),
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(err = %e, "IO error when trying to fallback to wkg.toml");
                }
            }
        };
    }

    toml_project_config
        .convert(wasmcloud_toml_dir)
        .map_err(|e: anyhow::Error| anyhow!("{} in {}", e, wasmcloud_toml_path.display()))
}

/// The wasmcloud.toml specification format as de-serialization friendly project configuration data
///
/// This structure is normally directly de-serialized from `wasmcloud.toml`,
/// and is used to build a more structured [`ProjectConfig`] object.
///
/// Below is an example of each option in the wasmcloud.toml file. A real example
/// only needs to include the fields that are relevant to the project.
///
/// ```rust
/// use crate::lib::parser::WasmcloudDotToml;
///
/// let component_toml = r#"
/// language = "rust"
/// type = "component"
/// name = "testcomponent"
/// version = "0.1.0"
/// "#;
/// let config: WasmcloudDotToml = toml::from_str(component_toml).expect("should deserialize");
/// eprintln!("{config:?}");
/// ```
#[derive(Deserialize, Debug)]
pub struct WasmcloudDotToml {
    /// The language of the project, e.g. rust, tinygo. This is used to determine which config to parse.
    pub language: String,

    /// The type of project. This is a string that is used to determine which type of config to parse.
    /// The toml file name is just "type" but is named `project_type` here to avoid clashing with the type keyword in Rust.
    #[serde(rename = "type")]
    pub project_type: String,

    /// Name of the project. Optional if building a Rust project, as it can be inferred from Cargo.toml.
    pub name: Option<String>,

    /// Semantic version of the project. Optional if building a Rust project, as it can be inferred from Cargo.toml.
    pub version: Option<Version>,

    /// Monotonically increasing revision number.
    #[serde(default)]
    pub revision: i32,

    /// Path to the directory where the project is located. Defaults to the current directory.
    /// This path is where build commands will be run.
    pub path: Option<PathBuf>,

    /// Path to the directory where the WIT world and dependencies can be found. Defaults to a `wit`
    /// directory in the project root.
    pub wit: Option<PathBuf>,

    /// Path to the directory where the built artifacts should be written. Defaults to a `build`
    /// directory in the project root.
    pub build: Option<PathBuf>,

    /// Configuration relevant to components
    #[serde(default)]
    pub component: ComponentConfig,

    /// Configuration relevant to providers
    #[serde(default)]
    pub provider: ProviderConfig,

    /// Rust configuration and options
    #[serde(default)]
    pub rust: RustConfig,

    /// `TinyGo` related configuration and options
    #[serde(default)]
    pub tinygo: TinyGoConfig,

    /// Golang related configuration and options
    #[serde(default)]
    pub go: GoConfig,

    /// Configuration for development environments and/or DX related plugins
    #[serde(default)]
    pub dev: DevConfig,

    /// Overrides for interface dependencies.
    ///
    /// This is often used to point to local wit files
    #[serde(flatten)]
    pub package_config: Option<PackageConfig>,

    /// Configuration for image registry usage
    #[serde(default)]
    pub registry: RegistryConfig,
}

impl WasmcloudDotToml {
    // Given a path to a valid cargo project, build an common_config enriched with Rust-specific information
    fn build_common_config_from_cargo_project(
        project_dir: PathBuf,
        build_dir: PathBuf,
        wit_dir: PathBuf,
        name: Option<String>,
        version: Option<Version>,
        revision: i32,
        registry: RegistryConfig,
    ) -> Result<CommonConfig> {
        let cargo_toml_path = project_dir.join("Cargo.toml");
        if !cargo_toml_path.is_file() {
            bail!(
                "missing/invalid Cargo.toml path [{}]",
                cargo_toml_path.display(),
            );
        }

        // Build the manifest
        let mut cargo_toml = Manifest::from_path(cargo_toml_path)?;

        // Populate Manifest with lib/bin information
        cargo_toml.complete_from_path(&project_dir)?;

        let cargo_pkg = cargo_toml
            .package
            .ok_or_else(|| anyhow!("Missing package information in Cargo.toml"))?;

        let version = match version {
            Some(version) => version,
            None => Version::parse(cargo_pkg.version.get()?.as_str())?,
        };

        let name = name.unwrap_or(cargo_pkg.name);

        // Determine the wasm module name from the [lib] section of Cargo.toml
        let wasm_bin_name = match cargo_toml.lib {
            Some(Product {
                name: Some(lib_name),
                ..
            }) => Some(lib_name),
            _ => None,
        };

        Ok(CommonConfig {
            name,
            version,
            revision,
            project_dir,
            build_dir,
            wit_dir,
            wasm_bin_name,
            registry,
        })
    }

    pub fn convert(self, wasmcloud_toml_dir: PathBuf) -> Result<ProjectConfig> {
        let project_type_config = match self.project_type.trim().to_lowercase().as_str() {
            "component" => TypeConfig::Component(self.component),
            "provider" => TypeConfig::Provider(self.provider),
            project_type => bail!("unknown project type: {project_type}"),
        };

        let language_config = match self.language.trim().to_lowercase().as_str() {
            "rust" => LanguageConfig::Rust(self.rust),
            "go" => LanguageConfig::Go(self.go),
            "tinygo" => LanguageConfig::TinyGo(self.tinygo),
            other => LanguageConfig::Other(other.to_string()),
        };

        // Use the provided `path` in the wasmcloud.toml file, or default to the current directory
        let project_path = self
            .path
            .map(|p| {
                // If the path in the wasmcloud.toml is absolute, use that directly.
                // Otherwise, join it with the project_path so that it's relative to the wasmcloud.toml
                if p.is_absolute() {
                    p
                } else {
                    wasmcloud_toml_dir.join(p)
                }
            })
            .unwrap_or_else(|| wasmcloud_toml_dir.clone());
        let project_path = project_path.canonicalize().with_context(|| {
            format!(
                "failed to canonicalize project path, ensure it exists: [{}]",
                project_path.display()
            )
        })?;
        let build_dir = self
            .build
            .map(|build_dir| {
                if build_dir.is_absolute() {
                    Ok(build_dir)
                } else {
                    // The build_dir is relative to the wasmcloud.toml file, so we need to join it with the wasmcloud_toml_dir
                    canonicalize_or_create(wasmcloud_toml_dir.join(build_dir.as_path()))
                }
            })
            .unwrap_or_else(|| Ok(project_path.join("build")))?;
        let wit_dir = self
            .wit
            .map(|wit_dir| {
                if wit_dir.is_absolute() {
                    Ok(wit_dir)
                } else {
                    // The wit_dir is relative to the wasmcloud.toml file, so we need to join it with the wasmcloud_toml_dir
                    wasmcloud_toml_dir
                        .join(wit_dir.as_path())
                        .canonicalize()
                        .with_context(|| {
                            format!(
                                "failed to canonicalize wit directory, ensure it exists: [{}]",
                                wit_dir.display()
                            )
                        })
                }
            })
            .unwrap_or_else(|| Ok(project_path.join("wit")))?;

        let common_config = match language_config {
            LanguageConfig::Rust(_) => {
                match Self::build_common_config_from_cargo_project(
                    project_path.clone(),
                    build_dir.clone(),
                    wit_dir.clone(),
                    self.name.clone(),
                    self.version.clone(),
                    self.revision,
                    self.registry.clone(),
                ) {
                    // Successfully built with cargo information
                    Ok(cfg) => cfg,

                    // Fallback to non-specific language usage if we at least have a name & version
                    Err(_) if self.name.is_some() && self.version.is_some() => CommonConfig {
                        name: self.name.unwrap(),
                        version: self.version.unwrap(),
                        revision: self.revision,
                        wasm_bin_name: None,
                        project_dir: project_path,
                        wit_dir,
                        build_dir,
                        registry: self.registry,
                    },

                    Err(err) => {
                        bail!("No Cargo.toml file found in the current directory, and name/version unspecified: {err}")
                    }
                }
            }

            LanguageConfig::Go(_) | LanguageConfig::TinyGo(_) | LanguageConfig::Other(_) => {
                CommonConfig {
                    name: self
                        .name
                        .ok_or_else(|| anyhow!("Missing name in wasmcloud.toml"))?,
                    version: self
                        .version
                        .ok_or_else(|| anyhow!("Missing version in wasmcloud.toml"))?,
                    revision: self.revision,
                    project_dir: project_path,
                    wasm_bin_name: None,
                    wit_dir,
                    build_dir,
                    registry: self.registry,
                }
            }
        };

        let package_config = self
            .package_config
            .map(|mut package_config| {
                package_config.overrides = package_config.overrides.map(|overrides| {
                    // Each override can contain an absolute path or a relative (to the wasmcloud.toml) path to a local
                    // set of WIT dependencies. To support running the build process from anywhere, we need to canonicalize
                    // these paths.
                    overrides
                        .into_iter()
                        .map(|(k, mut v)| {
                            if let Some(path) = v.path.as_ref() {
                                trace!("canonicalizing override path: [{}]", path.display());
                                let path = if path.is_absolute() {
                                    path.clone()
                                } else {
                                    let override_path = wasmcloud_toml_dir.join(path);
                                    override_path.canonicalize().unwrap_or_else(|e| {
                                        warn!(
                                            ?e,
                                            "failed to canonicalize override path, falling back to: [{}]",
                                            override_path.display()
                                        );
                                        override_path
                                    })
                                };
                                v.path = Some(path);
                            }
                            (k, v)
                        })
                        .collect::<HashMap<String, Override>>()
                });
                package_config
            })
            .unwrap_or_default();

        Ok(ProjectConfig {
            dev: self.dev,
            project_type: project_type_config,
            language: language_config,
            common: common_config,
            package_config,
            wasmcloud_toml_dir,
        })
    }
}

/// Project configuration, normally specified in the root keys of a wasmcloud.toml file
#[derive(Deserialize, Debug, Clone)]
pub struct ProjectConfig {
    /// The language of the project, e.g. rust, tinygo. Contains specific configuration for that language.
    pub language: LanguageConfig,
    /// The type of project, e.g. component, provider, interface. Contains the specific configuration for that type.
    /// This is renamed to "type" but is named `project_type` here to avoid clashing with the type keyword in Rust.
    #[serde(rename = "type")]
    pub project_type: TypeConfig,
    /// Configuration common among all project types & languages.
    pub common: CommonConfig,
    /// Configuration for development environments and/or DX related plugins
    pub dev: DevConfig,
    /// Configuration for package tooling
    pub package_config: PackageConfig,
    /// The directory where the project wasmcloud.toml file is located
    #[serde(skip)]
    pub wasmcloud_toml_dir: PathBuf,
}

impl ProjectConfig {
    pub fn resolve_registry_credentials(
        &self,
        registry: impl AsRef<str>,
    ) -> Result<RegistryCredential> {
        let credentials_file = &self.common.registry.push.credentials.clone();

        let Some(credentials_file) = credentials_file else {
            bail!("No registry credentials path configured")
        };

        if !credentials_file.exists() {
            bail!(
                "Provided registry credentials file ({}) does not exist",
                credentials_file.display()
            )
        }

        let credentials = std::fs::read_to_string(credentials_file).with_context(|| {
            format!(
                "Failed to read registry credentials file {}",
                credentials_file.display()
            )
        })?;

        let credentials = serde_json::from_str::<HashMap<String, RegistryCredential>>(&credentials)
            .with_context(|| {
                format!(
                    "Failed to parse registry credentials from file {}",
                    credentials_file.display()
                )
            })?;

        let Some(credentials) = credentials.get(registry.as_ref()) else {
            bail!(
                "Unable to find credentials for {} in the configured registry credentials file",
                registry.as_ref()
            )
        };

        Ok(credentials.clone())
    }
}

/// Helper function to canonicalize a path or create it if it doesn't exist before
/// attempting to canonicalize it. This is a nice helper to ensure that we can attempt
/// to precreate directories like `build` before we start writing to them.
fn canonicalize_or_create(path: PathBuf) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&path).with_context(|| {
                format!(
                    "failed to create directory [{}] before canonicalizing",
                    path.display()
                )
            })?;
            path.canonicalize().with_context(|| {
                format!(
                    "failed to canonicalize directory [{}] after creating it",
                    path.display()
                )
            })
        }
        Err(e) => {
            Err(e).with_context(|| format!("failed to canonicalize directory [{}]", path.display()))
        }
    }
}

// TODO(joonas): Remove these once doctests are run as part of CI.
#[cfg(test)]
mod tests {
    use crate::lib::parser::WitInterfaceSpec;
    use std::str::FromStr;

    #[test]
    fn test_includes() {
        let wasi_http = WitInterfaceSpec::from_str("wasi:http")
            .expect("should parse 'wasi:http' into WitInterfaceSpec");
        let wasi_http_incoming_handler = WitInterfaceSpec::from_str("wasi:http/incoming-handler")
            .expect("should parse 'wasi:http/incoming-handler' into WitInterfaceSpec");
        let wasi_http_incoming_handler_handle =
            WitInterfaceSpec::from_str("wasi:http/incoming-handler.handle")
                .expect("should parse 'wasi:http/incoming-handler.handle' into WitInterfaceSpec");
        assert!(wasi_http.includes(&wasi_http_incoming_handler));
        assert!(wasi_http_incoming_handler.includes(&wasi_http_incoming_handler_handle));
    }
}
