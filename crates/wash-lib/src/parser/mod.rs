//! Parse wasmcloud.toml files which specify key information for building and signing
//! WebAssembly modules and native capability provider binaries

use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    fs,
    path::PathBuf,
};

use anyhow::{anyhow, bail, Context, Result};
use cargo_toml::{Manifest, Product};
use config::Config;
use semver::Version;
use serde::Deserialize;
use wasmcloud_control_interface::RegistryCredential;

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

/// Project configuration, normally specified in the root keys of a wasmcloud.toml file
#[derive(Deserialize, Debug, Clone)]
pub struct ProjectConfig {
    /// The language of the project, e.g. rust, tinygo. Contains specific configuration for that language.
    pub language: LanguageConfig,
    /// The type of project, e.g. component, provider, interface. Contains the specific configuration for that type.
    /// This is renamed to "type" but is named project_type here to avoid clashing with the type keyword in Rust.
    #[serde(rename = "type")]
    pub project_type: TypeConfig,
    /// Configuration common among all project types & languages.
    pub common: CommonConfig,
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct ComponentConfig {
    /// The list of provider claims that this component requires. eg. ["wasmcloud:httpserver", "wasmcloud:blobstore"]
    pub claims: Vec<String>,
    /// Whether to push to the registry insecurely. Defaults to false.
    pub push_insecure: bool,
    /// The directory to store the private signing keys in.
    pub key_directory: PathBuf,
    /// The call alias of the component.
    pub call_alias: Option<String>,
    /// The target wasm target to build for. Defaults to "wasm32-unknown-unknown" (a WASM core module).
    pub wasm_target: WasmTarget,
    /// Path to a wasm adapter that can be used for preview2
    pub wasi_preview2_adapter_path: Option<PathBuf>,
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

impl RustConfig {
    #[must_use]
    pub fn build_target(&self, wasm_target: &WasmTarget) -> &'static str {
        match wasm_target {
            WasmTarget::CoreModule => "wasm32-unknown-unknown",
            // NOTE: eventually "wasm32-wasi" will be renamed to "wasm32-wasi-preview1"
            // https://github.com/rust-lang/compiler-team/issues/607
            WasmTarget::WasiPreview1 | WasmTarget::WasiPreview2 => "wasm32-wasi",
        }
    }
}

#[derive(Deserialize, Debug, PartialEq)]
struct RawComponentConfig {
    /// The list of provider claims that this component requires. eg. ["wasmcloud:httpserver", "wasmcloud:blobstore"]
    pub claims: Option<Vec<String>>,
    /// Whether to push to the registry insecurely. Defaults to false.
    pub push_insecure: Option<bool>,
    /// The directory to store the private signing keys in. Defaults to "./keys".
    pub key_directory: Option<PathBuf>,
    /// The target wasm target to build for. Defaults to "wasm32-unknown-unknown".
    pub wasm_target: Option<String>,
    /// Path to a wasm adapter that can be used for preview2
    pub wasi_preview2_adapter_path: Option<PathBuf>,
    /// The call alias of the component. Defaults to no alias.
    pub call_alias: Option<String>,
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

impl TryFrom<RawComponentConfig> for ComponentConfig {
    type Error = anyhow::Error;

    fn try_from(raw_config: RawComponentConfig) -> Result<Self> {
        let key_directory = if let Some(key_directory) = raw_config.key_directory {
            key_directory
        } else {
            let home_dir = dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Unable to determine the user's home directory"))?;
            home_dir.join(".wash/keys")
        };
        Ok(Self {
            claims: raw_config.claims.unwrap_or_default(),
            push_insecure: raw_config.push_insecure.unwrap_or(false),
            key_directory,
            wasm_target: raw_config
                .wasm_target
                .map(WasmTarget::from)
                .unwrap_or_default(),
            wasi_preview2_adapter_path: raw_config.wasi_preview2_adapter_path,
            call_alias: raw_config.call_alias,
            wit_world: raw_config.wit_world,
            tags: raw_config.tags,
            build_command: raw_config.build_command,
            build_artifact: raw_config.build_artifact,
            destination: raw_config.destination,
        })
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct ProviderConfig {
    /// The vendor name of the provider.
    pub vendor: String,
    /// Optional WIT world for the provider, e.g. `wasmcloud:messaging`
    pub wit_world: Option<String>,
    /// The target operating system of the provider archive. Defaults to the current OS.
    pub os: String,
    /// The target architecture of the provider archive. Defaults to the current architecture.
    pub arch: String,
    /// The Rust target triple to build for. Defaults to the default rust toolchain.
    pub rust_target: Option<String>,
    /// Optional override for the provider binary name, required if we cannot infer this from Cargo.toml
    pub bin_name: Option<String>,
    /// The directory to store the private signing keys in.
    pub key_directory: PathBuf,
}

#[derive(Deserialize, Debug, PartialEq)]
struct RawProviderConfig {
    /// The vendor name of the provider.
    pub vendor: Option<String>,
    /// Optional WIT world for the provider, e.g. `wasmcloud:messaging`
    pub wit_world: Option<String>,
    /// The target operating system of the provider archive. Defaults to the current OS.
    pub os: Option<String>,
    /// The target architecture of the provider archive. Defaults to the current architecture.
    pub arch: Option<String>,
    /// The Rust target triple to build for. Defaults to the default rust toolchain.
    pub rust_target: Option<String>,
    /// Optional override for the provider binary name, required if we cannot infer this from Cargo.toml
    pub bin_name: Option<String>,
    /// The directory to store the private signing keys in.
    pub key_directory: Option<PathBuf>,
}

impl TryFrom<RawProviderConfig> for ProviderConfig {
    type Error = anyhow::Error;

    fn try_from(raw_config: RawProviderConfig) -> Result<Self> {
        let key_directory = if let Some(key_directory) = raw_config.key_directory {
            key_directory
        } else {
            let home_dir = dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("Unable to determine the user's home directory"))?;
            home_dir.join(".wash/keys")
        };
        Ok(Self {
            vendor: raw_config.vendor.unwrap_or_else(|| "NoVendor".to_string()),
            os: raw_config
                .os
                .unwrap_or_else(|| std::env::consts::OS.to_string()),
            arch: raw_config
                .arch
                .unwrap_or_else(|| std::env::consts::ARCH.to_string()),
            rust_target: raw_config.rust_target,
            bin_name: raw_config.bin_name,
            wit_world: raw_config.wit_world,
            key_directory,
        })
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct RustConfig {
    /// The path to the cargo binary. Optional, will default to search the user's `PATH` for `cargo` if not specified.
    pub cargo_path: Option<PathBuf>,
    /// Path to cargo/rust's `target` directory. Optional, defaults to the cargo target directory for the workspace or project.
    pub target_path: Option<PathBuf>,
    // Whether to build in debug mode. Defaults to false.
    pub debug: bool,
}

#[derive(Deserialize, Debug, PartialEq, Default, Clone)]
struct RawRustConfig {
    /// The path to the cargo binary. Optional, will default to search the user's `PATH` for `cargo` if not specified.
    pub cargo_path: Option<PathBuf>,
    /// Path to cargo/rust's `target` directory. Optional, defaults to `./target`.
    pub target_path: Option<PathBuf>,
    // Whether to build in debug mode. Defaults to false.
    pub debug: bool,
}

impl TryFrom<RawRustConfig> for RustConfig {
    type Error = anyhow::Error;

    fn try_from(raw_config: RawRustConfig) -> Result<Self> {
        Ok(Self {
            cargo_path: raw_config.cargo_path,
            target_path: raw_config.target_path,
            debug: raw_config.debug,
        })
    }
}

#[derive(Deserialize, Debug, PartialEq, Default, Clone)]
struct RawRegistryConfig {
    url: Option<String>,
    credentials: Option<PathBuf>,
}
#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct RegistryConfig {
    pub url: Option<String>,
    pub credentials: Option<PathBuf>,
}

impl TryFrom<RawRegistryConfig> for RegistryConfig {
    type Error = anyhow::Error;

    fn try_from(raw_config: RawRegistryConfig) -> Result<Self> {
        Ok(Self {
            url: raw_config.url,
            credentials: raw_config.credentials,
        })
    }
}
/// Configuration common amoung all project types & languages.
#[derive(Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct CommonConfig {
    /// Name of the project.
    pub name: String,
    /// Semantic version of the project.
    pub version: Version,
    /// Monotonically increasing revision number
    pub revision: i32,
    /// Path to the project directory to determine where built and signed artifacts should be
    pub path: PathBuf,
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
    #[serde(alias = "wasm32-wasi", alias = "wasm32-wasi-preview1")]
    WasiPreview1,
    #[serde(alias = "wasm32-wasi-preview2")]
    WasiPreview2,
}

impl From<&str> for WasmTarget {
    fn from(value: &str) -> Self {
        match value {
            "wasm32-wasi-preview1" => WasmTarget::WasiPreview1,
            "wasm32-wasi" => WasmTarget::WasiPreview1,
            "wasm32-wasi-preview2" => WasmTarget::WasiPreview2,
            _ => WasmTarget::CoreModule,
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
            WasmTarget::CoreModule => "wasm32-unknown-unknown",
            WasmTarget::WasiPreview1 => "wasm32-wasi",
            WasmTarget::WasiPreview2 => "wasm32-wasi-preview2",
        })
    }
}

#[derive(Deserialize, Debug)]
struct RawProjectConfig {
    /// The language of the project, e.g. rust, tinygo. This is used to determine which config to parse.
    pub language: String,

    /// The type of project. This is a string that is used to determine which type of config to parse.
    /// The toml file name is just "type" but is named project_type here to avoid clashing with the type keyword in Rust.
    #[serde(rename = "type")]
    pub project_type: String,

    /// Name of the project.
    pub name: Option<String>,

    /// Semantic version of the project.
    pub version: Option<Version>,

    /// Monotonically increasing revision number.
    #[serde(default)]
    pub revision: i32,

    #[serde(alias = "component")]
    pub component: Option<RawComponentConfig>,
    pub provider: Option<RawProviderConfig>,

    pub rust: Option<RawRustConfig>,
    pub tinygo: Option<RawTinyGoConfig>,
    pub go: Option<RawGoConfig>,
    pub registry: Option<RawRegistryConfig>,
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct GoConfig {
    /// The path to the go binary. Optional, will default to `go` if not specified.
    pub go_path: Option<PathBuf>,
    /// Whether to disable the `go generate` step in the build process. Defaults to false.
    pub disable_go_generate: bool,
}

#[derive(Deserialize, Debug, PartialEq, Default)]
struct RawGoConfig {
    /// The path to the go binary. Optional, will default to `go` if not specified.
    pub go_path: Option<PathBuf>,
    /// Whether to disable the `go generate` step in the build process. Defaults to false.
    #[serde(default)]
    pub disable_go_generate: bool,
}

impl TryFrom<RawGoConfig> for GoConfig {
    type Error = anyhow::Error;

    fn try_from(raw: RawGoConfig) -> Result<Self> {
        Ok(Self {
            go_path: raw.go_path,
            disable_go_generate: raw.disable_go_generate,
        })
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct TinyGoConfig {
    /// The path to the tinygo binary. Optional, will default to `tinygo` if not specified.
    pub tinygo_path: Option<PathBuf>,
    /// Whether to disable the `go generate` step in the build process. Defaults to false.
    pub disable_go_generate: bool,
}

impl TinyGoConfig {
    #[must_use]
    pub fn build_target(&self, wasm_target: &WasmTarget) -> &'static str {
        match wasm_target {
            WasmTarget::CoreModule => "wasm",
            WasmTarget::WasiPreview1 | WasmTarget::WasiPreview2 => "wasi",
        }
    }
}

#[derive(Deserialize, Debug, PartialEq, Default)]
struct RawTinyGoConfig {
    /// The path to the tinygo binary. Optional, will default to `tinygo` if not specified.
    pub tinygo_path: Option<PathBuf>,
    /// Whether to disable the `go generate` step in the build process. Defaults to false.
    #[serde(default)]
    pub disable_go_generate: bool,
}

impl TryFrom<RawTinyGoConfig> for TinyGoConfig {
    type Error = anyhow::Error;

    fn try_from(raw: RawTinyGoConfig) -> Result<Self> {
        Ok(Self {
            tinygo_path: raw.tinygo_path,
            disable_go_generate: raw.disable_go_generate,
        })
    }
}

/// Gets the wasmCloud project (component, provider, or interface) config.
///
/// The config can come from multiple sources: a specific toml file path, a folder with a `wasmcloud.toml` file inside it, or by default it looks for a `wasmcloud.toml` file in the current directory.
///
/// The user can also override the config file by setting environment variables with the prefix "WASMCLOUD_". This behavior can be disabled by setting `use_env` to false.
/// For example, a user could set the variable `WASMCLOUD_RUST_CARGO_PATH` to override the default `cargo` path.
///
/// # Arguments
/// * `opt_path` - The path to the config file. If None, it will look for a wasmcloud.toml file in the current directory.
/// * `use_env` - Whether to use the environment variables or not. If false, it will not attempt to use environment variables. Defaults to true.
pub fn get_config(opt_path: Option<PathBuf>, use_env: Option<bool>) -> Result<ProjectConfig> {
    let mut path = opt_path.unwrap_or_else(|| PathBuf::from("."));

    if !path.exists() {
        bail!("path {} does not exist", path.display());
    }

    path = fs::canonicalize(path)?;
    let (project_path, wasmcloud_path) = if path.is_dir() {
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

    let mut config = Config::builder().add_source(config::File::from(wasmcloud_path.clone()));

    if use_env.unwrap_or(true) {
        config = config.add_source(config::Environment::with_prefix("WASMCLOUD"));
    }

    let json_value = config
        .build()
        .map_err(|e| {
            if e.to_string().contains("is not of a registered file format") {
                return anyhow!("invalid config file: {}", wasmcloud_path.display());
            }

            anyhow!("{}", e)
        })?
        .try_deserialize::<serde_json::Value>()?;

    let raw_project_config: RawProjectConfig = serde_json::from_value(json_value)?;

    raw_project_config
        .convert(project_path)
        .map_err(|e: anyhow::Error| anyhow!("{} in {}", e, wasmcloud_path.display()))
}

impl RawProjectConfig {
    // Given a path to a valid cargo project, build an common_config enriched with Rust-specific information
    fn build_common_config_from_cargo_project(
        project_path: PathBuf,
        name: Option<String>,
        version: Option<Version>,
        revision: i32,
        registry: RegistryConfig,
    ) -> Result<CommonConfig> {
        let cargo_toml_path = project_path.join("Cargo.toml");
        if !cargo_toml_path.is_file() {
            bail!(
                "missing/invalid Cargo.toml path [{}]",
                cargo_toml_path.display(),
            );
        }

        // Build the manifest
        let mut cargo_toml = Manifest::from_path(cargo_toml_path)?;

        // Populate Manifest with lib/bin information
        cargo_toml.complete_from_path(&project_path)?;

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
            path: project_path,
            wasm_bin_name,
            registry,
        })
    }

    pub fn convert(self, project_path: PathBuf) -> Result<ProjectConfig> {
        let project_type_config = match self.project_type.trim().to_lowercase().as_str() {
            "component" => {
                let component_config = self.component.context("missing component config")?;
                TypeConfig::Component(component_config.try_into()?)
            }

            "provider" => TypeConfig::Provider(
                self.provider
                    .context("missing provider config")?
                    .try_into()?,
            ),

            _ => {
                bail!("unknown project type: {}", self.project_type);
            }
        };

        let language_config = match self.language.trim().to_lowercase().as_str() {
            "rust" => match self.rust {
                Some(rust_config) => LanguageConfig::Rust(rust_config.try_into()?),
                None => LanguageConfig::Rust(RustConfig::default()),
            },
            "go" => match self.go {
                Some(go_config) => LanguageConfig::Go(go_config.try_into()?),
                None => LanguageConfig::Go(GoConfig::default()),
            },
            "tinygo" => match self.tinygo {
                Some(tinygo_config) => LanguageConfig::TinyGo(tinygo_config.try_into()?),
                None => LanguageConfig::TinyGo(TinyGoConfig::default()),
            },
            other => LanguageConfig::Other(other.to_string()),
        };

        let registry_config = self
            .registry
            .map(RegistryConfig::try_from)
            .transpose()?
            .unwrap_or_default();

        let common_config_result: Result<CommonConfig> = match language_config {
            LanguageConfig::Rust(_) => {
                match Self::build_common_config_from_cargo_project(
                    project_path.clone(),
                    self.name.clone(),
                    self.version.clone(),
                    self.revision,
                    registry_config.clone(),
                ) {
                    // Successfully built with cargo information
                    Ok(cfg) => Ok(cfg),

                    // Fallback to non-specific language usage if we at least have a name & version
                    Err(_) if self.name.is_some() && self.version.is_some() => Ok(CommonConfig {
                        name: self.name.unwrap(),
                        version: self.version.unwrap(),
                        revision: self.revision,
                        path: project_path,
                        wasm_bin_name: None,
                        registry: registry_config,
                    }),

                    Err(err) => {
                        bail!("No Cargo.toml file found in the current directory, and name/version unspecified: {err}")
                    }
                }
            }

            LanguageConfig::Go(_) | LanguageConfig::TinyGo(_) | LanguageConfig::Other(_) => {
                Ok(CommonConfig {
                    name: self
                        .name
                        .ok_or_else(|| anyhow!("Missing name in wasmcloud.toml"))?,
                    version: self
                        .version
                        .ok_or_else(|| anyhow!("Missing version in wasmcloud.toml"))?,
                    revision: self.revision,
                    path: project_path,
                    wasm_bin_name: None,
                    registry: registry_config,
                })
            }
        };

        Ok(ProjectConfig {
            language: language_config,
            project_type: project_type_config,
            common: common_config_result?,
        })
    }
}

impl ProjectConfig {
    pub async fn resolve_registry_credentials(
        &self,
        registry: impl AsRef<str>,
    ) -> Result<RegistryCredential> {
        let credentials_file = &self.common.registry.credentials.clone();

        let Some(credentials_file) = credentials_file else {
            return Ok(RegistryCredential::default());
        };

        if !credentials_file.exists() {
            return Ok(RegistryCredential::default());
        }

        let credentials = tokio::fs::read_to_string(&credentials_file)
            .await
            .context(format!(
                "Failed to read registry credentials file {}",
                credentials_file.display()
            ))?;

        let credentials = serde_json::from_str::<HashMap<String, RegistryCredential>>(&credentials)
            .context(format!(
                "Failed to parse registry credentials from file {}",
                credentials_file.display()
            ))?;

        let Some(credentials) = credentials.get(registry.as_ref()) else {
            return Ok(RegistryCredential::default());
        };

        Ok(credentials.clone())
    }
}
