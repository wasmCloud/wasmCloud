//! Contains the [Config] struct and related functions for managing
//! wash configuration, including loading, saving, and merging configurations
//! with explicit defaults.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use figment::{
    Figment,
    providers::{Env, Format, Json, Toml, Yaml},
};
use serde::{Deserialize, Serialize};
use tracing::info;
use wash_runtime::wit::WitInterface;

use crate::{
    cli::{CONFIG_DIR_NAME, CONFIG_FILE_NAME, VALID_CONFIG_FILES},
    wit::WitConfig,
};

/// Main wash configuration structure with hierarchical merging support and explicit defaults
///
/// The "global" [Config] is stored under the user's XDG_CONFIG_HOME directory
/// (typically `~/.config/wash/config.yaml`), while the "local" project configuration
/// is stored in the project's `.wash/config.yaml` file. This allows for both reasonable
/// global defaults and project-specific overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Version of the configuration schema (default: current Cargo package version)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Build configuration for different project types (default: empty/optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildConfig>,

    /// Wash dev configuration (default: empty/optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dev: Option<DevConfig>,

    /// Wash new configuration (default: empty/optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new: Option<NewConfig>,

    /// WIT dependency management configuration (default: empty/optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wit: Option<WitConfig>,
    // TODO(#15): Support dev config which can be overridden in local project config
    // e.g. for runtime config, http ports, etc
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            build: None,
            new: None,
            dev: None,
            wit: None,
        }
    }
}

impl Config {
    /// Get the WIT directory from the configuration, defaulting to "./wit" if not set
    pub fn wit_dir(&self) -> PathBuf {
        if let Some(wit_config) = &self.wit
            && let Some(wit_dir) = &wit_config.wit_dir
        {
            return wit_dir.clone();
        }
        PathBuf::from("wit")
    }

    /// Get the development configuration, defaulting to [DevConfig::default()] if not set
    pub fn dev(&self) -> DevConfig {
        self.dev.clone().unwrap_or_default()
    }

    pub fn build(&self) -> BuildConfig {
        self.build.clone().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NewConfig {
    /// Optional command to run after creating a new project
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// Configuration for building WebAssembly components
///
/// # Example
///
/// ```yaml
/// build:
///   command: cargo build --target wasm32-wasip2 --release
///   component_path: target/wasm32-wasip2/release/my_component.wasm
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuildConfig {
    /// Command to build the component
    pub command: Option<String>,
    /// Environment variables to set when running the build command
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    /// Expected path to the built Wasm component artifact
    /// If not specified, defaults to `<project-dir>.wasm`.
    /// Relative paths are resolved against the project directory.
    /// Exposed to build commands via `WASH_COMPONENT_PATH` env var.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevVolume {
    /// Host path to mount
    pub host_path: PathBuf,
    /// Guest path inside the dev environment
    pub guest_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevComponent {
    /// Name of the component
    pub name: String,
    /// Path to the component file
    pub file: PathBuf,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DevConfig {
    /// Command to run the component in dev mode
    /// If not specified, defaults to 'build.command'.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Address for the dev server to bind to (default: "0.0.0.0:8000")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,

    /// Whether the component under development should be treated as a service
    #[serde(default)]
    pub service: bool,
    /// Optional path to a wasm component to be used as a service
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_file: Option<PathBuf>,

    /// Additional components to load alongside the main component
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<DevComponent>,

    /// Volumes to mount into the dev environment
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<DevVolume>,

    /// Host interfaces configuration
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_interfaces: Vec<WitInterface>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_cert_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_key_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_ca_path: Option<PathBuf>,

    /// Enable WASI WebGPU support in the dev environment. Only supported on non-Windows platforms.
    #[serde(default)]
    pub wasi_webgpu: bool,

    /// Optional Redis connection URL for the WASI keyvalue plugin.
    /// Example: redis://127.0.0.1:6379
    /// When set, takes precedence over wasi_keyvalue_path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasi_keyvalue_redis_url: Option<String>,

    /// Optional path for WASI keyvalue filesystem storage. If not set, an in-memory store is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasi_keyvalue_path: Option<PathBuf>,

    /// Optional NATS connection URL for the WASI keyvalue plugin.
    /// Example: nats://127.0.0.1:4222
    /// When set, takes precedence over wasi_keyvalue_path but is overridden by wasi_keyvalue_redis_url.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasi_keyvalue_nats_url: Option<String>,

    /// Optional path for WASI blobstore filesystem storage. If not set, an in-memory store is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasi_blobstore_path: Option<PathBuf>,

    /// Optional PostgreSQL connection URL for the wasmcloud:postgres plugin.
    /// Example: postgres://user:pass@bouncer:6432?sslmode=require&pool_size=10
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postgres_url: Option<String>,

    /// Enable WASI OpenTelemetry support
    #[serde(default)]
    pub wasi_otel: bool,

    /// Enable WASIP3 support for components that target wasi@0.3 interfaces
    #[serde(default)]
    pub wasip3: bool,
}

/// Load configuration with hierarchical merging
/// Order of precedence (lowest to highest):
/// 1. Default values
/// 2. Global config (~/.wash/config.yaml)
/// 3. Local project config (.wash/config.yaml)
/// 4. Environment variables (WASH_ prefix)
/// 5. Command line arguments
///
/// # Arguments
/// - `global_config_path`:
pub fn load_config<T>(
    global_config_path: &Path,
    project_dir: Option<&Path>,
    cli_args: Option<T>,
) -> Result<Config>
where
    T: Serialize + Into<Config>,
{
    let mut figment = Figment::new();

    // Start with defaults
    figment = figment.merge(figment::providers::Serialized::defaults(Config::default()));

    // Global config file
    if global_config_path.exists() {
        figment = figment.merge(load_config_file(global_config_path)?);
    }

    // Local project config
    if let Some(project_dir) = project_dir {
        let project_config_path = locate_project_config(project_dir);
        if project_config_path.exists() {
            figment = figment.merge(load_config_file(&project_config_path)?);
        }
    }

    // Environment variables with WASH_ prefix
    figment = figment.merge(Env::prefixed("WASH_"));

    // TODO(#16): There's more testing to be done here to ensure that CLI args can override existing
    // config without replacing present values with empty values.
    if let Some(args) = cli_args {
        // Convert CLI args to configuration format
        let cli_config: Config = args.into();
        figment = figment.merge(figment::providers::Serialized::defaults(cli_config));
    }

    figment
        .extract()
        .context("Failed to load wash configuration")
}

pub fn locate_project_config(project_dir: &Path) -> PathBuf {
    for file_name in VALID_CONFIG_FILES.iter() {
        let config_path = project_dir.join(CONFIG_DIR_NAME).join(file_name);
        if config_path.exists() {
            return config_path;
        }
    }

    project_dir.join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME)
}

pub fn locate_user_config(dot_dir: &Path) -> PathBuf {
    for file_name in VALID_CONFIG_FILES.iter() {
        let config_path = dot_dir.join(file_name);
        if config_path.exists() {
            return config_path;
        }
    }

    dot_dir.join(CONFIG_FILE_NAME)
}

fn load_config_file(file_path: &Path) -> Result<Figment> {
    let mut figment = Figment::new();

    match file_path.extension().and_then(|s| s.to_str()) {
        Some("yaml") | Some("yml") => {
            figment = figment.merge(Yaml::file_exact(file_path));
        }
        Some("json") => {
            figment = figment.merge(Json::file_exact(file_path));
        }
        Some("toml") => {
            figment = figment.merge(Toml::file_exact(file_path));
        }
        Some(ext) => {
            bail!("Unsupported global config file extension: {}", ext);
        }
        None => {
            bail!(
                "Global config file has no extension: {}",
                file_path.display()
            );
        }
    }

    Ok(figment)
}

/// Save configuration to specified path
pub async fn save_config(config: &Config, path: &Path) -> Result<()> {
    // Ensure directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            format!(
                "Failed to create config directory: {parent}",
                parent = parent.display()
            )
        })?;
    }

    let yaml_config =
        serde_yaml_ng::to_string(config).context("Failed to serialize configuration")?;

    tokio::fs::write(path, yaml_config)
        .await
        .with_context(|| format!("failed to write config file: {}", path.display()))?;

    Ok(())
}

/// Get the local project configuration file path
pub fn local_config_path(project_dir: &Path) -> PathBuf {
    project_dir.join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME)
}

/// Generate a default configuration file with all explicit defaults
/// This is useful for `wash config init` command
pub async fn generate_default_config(path: &Path, force: bool) -> Result<()> {
    // Don't overwrite existing config unless force is specified
    if path.exists() && !force {
        bail!(
            "Configuration file already exists at {}. Use --force to overwrite",
            path.display()
        );
    }

    save_config(&Config::default(), path).await?;

    info!(config_path = %path.display(), "Generated default configuration");
    Ok(())
}
