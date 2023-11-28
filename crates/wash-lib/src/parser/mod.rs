//! Parse wasmcloud.toml files which specify key information for building and signing
//! WebAssembly modules and native capability provider binaries

use std::{collections::HashSet, fmt::Display, fs, path::PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use cargo_toml::{Manifest, Product};
use config::Config;
use oci_distribution::secrets::RegistryAuth;
use semver::Version;
use serde::Deserialize;
use wasmcloud_control_interface::RegistryCredentialMap;

#[derive(Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum LanguageConfig {
    Rust(RustConfig),
    TinyGo(TinyGoConfig),
}

#[allow(clippy::large_enum_variant)]
#[derive(Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum TypeConfig {
    Actor(ActorConfig),
    Provider(ProviderConfig),
    Interface(InterfaceConfig),
}

/// Project configuration, normally specified in the root keys of a wasmcloud.toml file
#[derive(Deserialize, Debug, Clone)]
pub struct ProjectConfig {
    /// The language of the project, e.g. rust, tinygo. Contains specific configuration for that language.
    pub language: LanguageConfig,
    /// The type of project, e.g. actor, provider, interface. Contains the specific configuration for that type.
    /// This is renamed to "type" but is named project_type here to avoid clashing with the type keyword in Rust.
    #[serde(rename = "type")]
    pub project_type: TypeConfig,
    /// Configuration common amoung all project types & languages.
    pub common: CommonConfig,
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct ActorConfig {
    /// The list of provider claims that this actor requires. eg. ["wasmcloud:httpserver", "wasmcloud:blobstore"]
    pub claims: Vec<String>,
    /// Whether to push to the registry insecurely. Defaults to false.
    pub push_insecure: bool,
    /// The directory to store the private signing keys in.
    pub key_directory: PathBuf,
    /// The call alias of the actor.
    pub call_alias: Option<String>,
    /// The target wasm target to build for. Defaults to "wasm32-unknown-unknown" (a WASM core module).
    pub wasm_target: WasmTarget,
    /// Path to a wasm adapter that can be used for preview2
    pub wasi_preview2_adapter_path: Option<PathBuf>,
    /// The WIT world that is implemented by the component
    pub wit_world: Option<String>,
    /// Tags that should be applied during the actor signing process
    pub tags: Option<HashSet<String>>,
    /// File path `wash` can use to find the built artifact. Defaults to `./build/[name].wasm`
    pub build_artifact: Option<PathBuf>,
    /// Optional build override command to run instead of attempting to use the native language
    /// toolchain to build. Keep in mind that `wash` expects for the built artifact to be located
    /// under the `build` directory of the project root unless overridden by `build_artifact`.
    pub build_command: Option<String>,
    /// File path the built and signed actor should be written to. Defaults to `./build/[name]_s.wasm`
    pub destination: Option<PathBuf>,
}

impl RustConfig {
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
struct RawActorConfig {
    /// The list of provider claims that this actor requires. eg. ["wasmcloud:httpserver", "wasmcloud:blobstore"]
    pub claims: Option<Vec<String>>,
    /// Whether to push to the registry insecurely. Defaults to false.
    pub push_insecure: Option<bool>,
    /// The directory to store the private signing keys in. Defaults to "./keys".
    pub key_directory: Option<PathBuf>,
    /// The target wasm target to build for. Defaults to "wasm32-unknown-unknown".
    pub wasm_target: Option<String>,
    /// Path to a wasm adapter that can be used for preview2
    pub wasi_preview2_adapter_path: Option<PathBuf>,
    /// The call alias of the actor. Defaults to no alias.
    pub call_alias: Option<String>,
    /// The WIT world that is implemented by the component
    pub wit_world: Option<String>,
    /// Tags that should be applied during the actor signing process
    pub tags: Option<HashSet<String>>,
    /// File path `wash` can use to find the built artifact. Defaults to `./build/[name].wasm`
    pub build_artifact: Option<PathBuf>,
    /// Optional build override command to run instead of attempting to use the native language
    /// toolchain to build. Keep in mind that `wash` expects for the built artifact to be located
    /// under the `build` directory of the project root unless overridden by `build_artifact`.
    pub build_command: Option<String>,
    /// File path the built and signed actor should be written to. Defaults to `./build/[name]_s.wasm`
    pub destination: Option<PathBuf>,
}

impl TryFrom<RawActorConfig> for ActorConfig {
    type Error = anyhow::Error;

    fn try_from(raw_config: RawActorConfig) -> Result<Self> {
        Ok(Self {
            claims: raw_config.claims.unwrap_or_default(),
            push_insecure: raw_config.push_insecure.unwrap_or(false),
            key_directory: raw_config
                .key_directory
                .unwrap_or_else(|| PathBuf::from("./keys")),
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
    /// The capability ID of the provider.
    pub capability_id: String,
    /// The vendor name of the provider.
    pub vendor: String,
}

#[derive(Deserialize, Debug, PartialEq)]
struct RawProviderConfig {
    /// The capability ID of the provider.
    pub capability_id: String,
    /// The vendor name of the provider. Optional, defaults to 'NoVendor'.
    pub vendor: Option<String>,
}

impl TryFrom<RawProviderConfig> for ProviderConfig {
    type Error = anyhow::Error;

    fn try_from(raw_config: RawProviderConfig) -> Result<Self> {
        Ok(Self {
            capability_id: raw_config.capability_id,
            vendor: raw_config.vendor.unwrap_or_else(|| "NoVendor".to_string()),
        })
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct InterfaceConfig {
    /// Directory to output HTML.
    pub html_target: PathBuf,
    /// Path to codegen.toml file.
    pub codegen_config: PathBuf,
}
#[derive(Deserialize, Debug, PartialEq)]

struct RawInterfaceConfig {
    /// Directory to output HTML. Defaults to "./html".
    pub html_target: Option<PathBuf>,
    /// Path to codegen.toml file. Optional, defaults to "./codegen.toml".
    pub codegen_config: Option<PathBuf>,
}

impl TryFrom<RawInterfaceConfig> for InterfaceConfig {
    type Error = anyhow::Error;

    fn try_from(raw_config: RawInterfaceConfig) -> Result<Self> {
        Ok(Self {
            html_target: raw_config
                .html_target
                .unwrap_or_else(|| PathBuf::from("./html")),
            codegen_config: raw_config
                .codegen_config
                .unwrap_or_else(|| PathBuf::from("./codegen.toml")),
        })
    }
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct RustConfig {
    /// The path to the cargo binary. Optional, will default to search the user's `PATH` for `cargo` if not specified.
    pub cargo_path: Option<PathBuf>,
    /// Path to cargo/rust's `target` directory. Optional, defaults to the cargo target directory for the workspace or project.
    pub target_path: Option<PathBuf>,
}

#[derive(Deserialize, Debug, PartialEq, Default, Clone)]
struct RawRustConfig {
    /// The path to the cargo binary. Optional, will default to search the user's `PATH` for `cargo` if not specified.
    pub cargo_path: Option<PathBuf>,
    /// Path to cargo/rust's `target` directory. Optional, defaults to `./target`.
    pub target_path: Option<PathBuf>,
}

impl TryFrom<RawRustConfig> for RustConfig {
    type Error = anyhow::Error;

    fn try_from(raw_config: RawRustConfig) -> Result<Self> {
        Ok(Self {
            cargo_path: raw_config.cargo_path,
            target_path: raw_config.target_path,
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

    pub actor: Option<RawActorConfig>,
    pub interface: Option<RawInterfaceConfig>,
    pub provider: Option<RawProviderConfig>,

    pub rust: Option<RawRustConfig>,
    pub tinygo: Option<RawTinyGoConfig>,
    pub registry: Option<RawRegistryConfig>,
}

#[derive(Deserialize, Debug, PartialEq, Eq, Clone, Default)]
pub struct TinyGoConfig {
    /// The path to the tinygo binary. Optional, will default to `tinygo` if not specified.
    pub tinygo_path: Option<PathBuf>,
}

impl TinyGoConfig {
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
}

impl TryFrom<RawTinyGoConfig> for TinyGoConfig {
    type Error = anyhow::Error;

    fn try_from(raw: RawTinyGoConfig) -> Result<Self> {
        Ok(Self {
            tinygo_path: raw.tinygo_path,
        })
    }
}

/// Gets the wasmCloud project (actor, provider, or interface) config.
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
            bail!("no wasmcloud.toml file found in {}", path.display());
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
        bail!("no wasmcloud.toml file found in {}", path.display());
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
            "actor" => {
                let actor_config = self.actor.context("missing actor config")?;
                TypeConfig::Actor(actor_config.try_into()?)
            }

            "provider" => TypeConfig::Provider(
                self.provider
                    .context("missing provider config")?
                    .try_into()?,
            ),

            "interface" => TypeConfig::Interface(
                self.interface
                    .context("missing interface config")?
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
            "tinygo" => match self.tinygo {
                Some(tinygo_config) => LanguageConfig::TinyGo(tinygo_config.try_into()?),
                None => LanguageConfig::TinyGo(TinyGoConfig::default()),
            },
            _ => {
                bail!("unknown language in wasmcloud.toml: {}", self.language);
            }
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

            LanguageConfig::TinyGo(_) => Ok(CommonConfig {
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
            }),
        };

        Ok(ProjectConfig {
            language: language_config,
            project_type: project_type_config,
            common: common_config_result?,
        })
    }
}

impl ProjectConfig {
    pub fn resolve_registry_url(&self) -> Result<String> {
        let registry_url = match &self.common.registry.url {
            Some(url) => url.clone(),
            None => {
                bail!("No registry URL specified in wasmcloud.toml");
            }
        };

        Ok(registry_url)
    }

    pub async fn resolve_registry_credentials(
        &self,
        registry: impl AsRef<str>,
    ) -> Result<RegistryAuth> {
        let credentials_file = &self.common.registry.credentials.to_owned();

        if credentials_file.is_none() {
            return Ok(RegistryAuth::Anonymous);
        }

        let credentials_file = credentials_file.clone().unwrap();

        if !credentials_file.exists() {
            return Ok(RegistryAuth::Anonymous);
        }

        let credentials = tokio::fs::read_to_string(&credentials_file).await;
        if credentials.is_err() {
            return Ok(RegistryAuth::Anonymous);
        }

        let credentials =
            serde_json::from_str::<RegistryCredentialMap>(&credentials.unwrap_or_default());
        if credentials.is_err() {
            return Ok(RegistryAuth::Anonymous);
        }

        let credentials = credentials.unwrap_or_default();
        if let Some(credentials) = credentials.get(registry.as_ref()) {
            match (credentials.username.clone(), credentials.password.clone()) {
                (Some(user), Some(password)) => {
                    return Ok(RegistryAuth::Basic(user, password));
                }
                _ => {
                    return Ok(RegistryAuth::Anonymous);
                }
            }
        }

        Ok(RegistryAuth::Anonymous)
    }
}
