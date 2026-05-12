//! Contains the [Config] struct and related functions for managing
//! wash configuration, including loading, saving, and merging configurations
//! with explicit defaults.

use std::{
    collections::{HashMap, HashSet},
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

    /// Validate the configuration by delegating to each section's own validator.
    ///
    /// All section errors are collected before returning so the caller sees every
    /// issue in a single `Err`. `project_dir` is used to resolve relative WIT source
    /// paths during validation.
    pub async fn validate(&self, project_dir: &Path) -> Result<()> {
        let mut errors: Vec<String> = Vec::new();

        if let Some(build) = &self.build
            && let Err(e) = build.validate()
        {
            errors.extend(e.to_string().lines().map(String::from));
        }
        if let Some(dev) = &self.dev
            && let Err(e) = dev.validate()
        {
            errors.extend(e.to_string().lines().map(String::from));
        }
        if let Some(wit) = &self.wit {
            match wit.validate(project_dir) {
                Ok(()) => {}
                Err(e) => errors.extend(e.to_string().lines().map(String::from)),
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            bail!("{}", errors.join("\n"))
        }
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

impl BuildConfig {
    pub fn validate(&self) -> Result<()> {
        if let Some(cmd) = &self.command
            && cmd.trim().is_empty()
        {
            bail!("build.command is empty");
        }
        Ok(())
    }
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

impl DevConfig {
    pub fn validate(&self) -> Result<()> {
        let mut errors: Vec<String> = Vec::new();

        if let Some(addr) = &self.address
            && addr.parse::<std::net::SocketAddr>().is_err()
        {
            errors.push(format!(
                "dev.address '{addr}' is not a valid host:port socket address"
            ));
        }

        match (self.tls_cert_path.is_some(), self.tls_key_path.is_some()) {
            (true, false) => {
                errors.push("dev.tls_cert_path is set but dev.tls_key_path is missing".to_string())
            }
            (false, true) => {
                errors.push("dev.tls_key_path is set but dev.tls_cert_path is missing".to_string())
            }
            _ => {}
        }

        if let Some(url) = &self.wasi_keyvalue_redis_url {
            check_url_scheme(
                "dev.wasi_keyvalue_redis_url",
                url,
                &["redis", "rediss"],
                &mut errors,
            );
        }
        if let Some(url) = &self.wasi_keyvalue_nats_url {
            check_url_scheme(
                "dev.wasi_keyvalue_nats_url",
                url,
                &["nats", "tls"],
                &mut errors,
            );
        }
        if let Some(url) = &self.postgres_url {
            check_url_scheme(
                "dev.postgres_url",
                url,
                &["postgres", "postgresql"],
                &mut errors,
            );
        }

        if cfg!(target_os = "windows") && self.wasi_webgpu {
            errors.push("dev.wasi_webgpu is not supported on Windows".to_string());
        }

        for comp in &self.components {
            if comp.name.trim().is_empty() {
                errors.push("dev.components contains an entry with empty name".to_string());
            }
            if comp.file.as_os_str().is_empty() {
                errors.push(format!("dev.components['{}'].file is empty", comp.name));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            bail!("{}", errors.join("\n"))
        }
    }
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

/// Parse a single config file at `path` and deserialize it into a [`Config`].
///
/// Unlike [`load_config`], this does not merge defaults, other config layers, or env
/// variables — it reflects exactly what is in the given file. Useful for validation.
pub fn load_config_from_file(path: &Path) -> Result<Config> {
    load_config_file(path)?
        .extract()
        .with_context(|| format!("failed to parse config from {}", path.display()))
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

pub async fn generate_default_config(path: &Path, force: bool) -> Result<()> {
    generate_config(&Config::default(), path, force).await
}

/// Generate an example configuration file with illustrative build/dev/wit sections,
/// useful for the `wash config init` command.
pub async fn generate_example_config(path: &Path, force: bool) -> Result<()> {
    generate_config(&example_config(), path, force).await
}

async fn generate_config(config: &Config, path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "Configuration file already exists at {}. Use --force to overwrite",
            path.display()
        );
    }

    let content = match path.extension().and_then(|e| e.to_str()) {
        Some("json") => {
            serde_json::to_string_pretty(config).context("failed to serialize config to JSON")?
        }
        Some("toml") => toml::to_string(config).context("failed to serialize config to TOML")?,
        _ => serde_yaml_ng::to_string(config).context("failed to serialize config to YAML")?,
    };

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create config directory: {}", parent.display()))?;
    }

    tokio::fs::write(path, content)
        .await
        .with_context(|| format!("failed to write config file: {}", path.display()))?;

    info!(config_path = %path.display(), "generated configuration");
    Ok(())
}

/// Build an example [`Config`] populated with sensible build, dev, and wit values.
pub fn example_config() -> Config {
    Config {
        version: Some(env!("CARGO_PKG_VERSION").to_string()),
        build: Some(BuildConfig {
            command: Some("cargo build --target wasm32-wasip2 --release".to_string()),
            env: HashMap::new(),
            component_path: Some(PathBuf::from(
                "target/wasm32-wasip2/release/component.wasm".to_string(),
            )),
        }),
        dev: Some(DevConfig {
            address: Some("0.0.0.0:8000".to_string()),
            service_file: Some(PathBuf::from("example/path/to/service.wasm")),
            components: vec![DevComponent {
                name: "example-sidecar".to_string(),
                file: PathBuf::from("example/path/to/sidecar.wasm"),
            }],
            volumes: vec![DevVolume {
                host_path: PathBuf::from("./data"),
                guest_path: PathBuf::from("/data"),
            }],
            host_interfaces: vec![WitInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interfaces: HashSet::from_iter(["incoming-handler".to_string()]),
                version: Some(semver::Version::new(0, 2, 0)),
                config: HashMap::new(),
                name: None,
            }],
            wasi_keyvalue_redis_url: Some("redis://127.0.0.1:6379".to_string()),
            wasi_keyvalue_path: Some(PathBuf::from("./data/keyvalue")),
            wasi_keyvalue_nats_url: Some("nats://127.0.0.1:4222".to_string()),
            wasi_blobstore_path: Some(PathBuf::from("./data/blobstore")),
            postgres_url: Some("postgres://user:pass@127.0.0.1:5432".to_string()),
            ..Default::default()
        }),
        new: None,
        wit: Some(WitConfig {
            registries: vec![],
            skip_fetch: false,
            wit_dir: Some(PathBuf::from("wit")),
            sources: HashMap::from_iter([
                (
                    "example:http".to_string(),
                    "https://example.com/wit.tar.gz".to_string(),
                ),
                (
                    "example:git".to_string(),
                    "git+https://github.com/user/repo.git".to_string(),
                ),
                (
                    "example:oci".to_string(),
                    "ghcr.io/user/package".to_string(),
                ),
            ]),
        }),
    }
}

fn check_url_scheme(field: &str, value: &str, expected: &[&str], errors: &mut Vec<String>) {
    match url::Url::parse(value) {
        Ok(u) if expected.contains(&u.scheme()) => {}
        Ok(u) => errors.push(format!(
            "{field} '{value}' has scheme '{}', expected one of: {}",
            u.scheme(),
            expected.join(", ")
        )),
        Err(e) => errors.push(format!("{field} '{value}' is not a valid URL: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_no_command_is_ok() {
        assert!(BuildConfig::default().validate().is_ok());
    }

    #[test]
    fn build_valid_command_is_ok() {
        let cfg = BuildConfig {
            command: Some("cargo build".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn build_empty_command_is_err() {
        let cfg = BuildConfig {
            command: Some("".to_string()),
            ..Default::default()
        };
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("build.command")
        );
    }

    #[test]
    fn build_whitespace_command_is_err() {
        let cfg = BuildConfig {
            command: Some("   ".to_string()),
            ..Default::default()
        };
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("build.command")
        );
    }

    #[test]
    fn dev_default_is_valid() {
        assert!(DevConfig::default().validate().is_ok());
    }

    #[test]
    fn dev_valid_address_is_ok() {
        let cfg = DevConfig {
            address: Some("0.0.0.0:8080".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn dev_invalid_address_is_err() {
        let cfg = DevConfig {
            address: Some("not-an-address".to_string()),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("dev.address"));
    }

    #[test]
    fn dev_tls_cert_without_key_is_err() {
        let cfg = DevConfig {
            tls_cert_path: Some("cert.pem".into()),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("tls_cert_path"));
    }

    #[test]
    fn dev_tls_key_without_cert_is_err() {
        let cfg = DevConfig {
            tls_key_path: Some("key.pem".into()),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("tls_key_path"));
    }

    #[test]
    fn dev_tls_both_set_is_ok() {
        let cfg = DevConfig {
            tls_cert_path: Some("cert.pem".into()),
            tls_key_path: Some("key.pem".into()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn dev_redis_wrong_scheme_is_err() {
        let cfg = DevConfig {
            wasi_keyvalue_redis_url: Some("http://localhost:6379".to_string()),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("wasi_keyvalue_redis_url"));
    }

    #[test]
    fn dev_redis_valid_scheme_is_ok() {
        let cfg = DevConfig {
            wasi_keyvalue_redis_url: Some("redis://127.0.0.1:6379".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn dev_rediss_valid_scheme_is_ok() {
        let cfg = DevConfig {
            wasi_keyvalue_redis_url: Some("rediss://127.0.0.1:6380".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn dev_nats_wrong_scheme_is_err() {
        let cfg = DevConfig {
            wasi_keyvalue_nats_url: Some("http://localhost:4222".to_string()),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("wasi_keyvalue_nats_url"));
    }

    #[test]
    fn dev_nats_valid_scheme_is_ok() {
        let cfg = DevConfig {
            wasi_keyvalue_nats_url: Some("nats://127.0.0.1:4222".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn dev_postgres_wrong_scheme_is_err() {
        let cfg = DevConfig {
            postgres_url: Some("mysql://localhost/db".to_string()),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("postgres_url"));
    }

    #[test]
    fn dev_postgres_valid_scheme_is_ok() {
        let cfg = DevConfig {
            postgres_url: Some("postgres://user:pass@localhost/db".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn dev_postgresql_valid_scheme_is_ok() {
        let cfg = DevConfig {
            postgres_url: Some("postgresql://user:pass@localhost/db".to_string()),
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn dev_component_empty_name_is_err() {
        let cfg = DevConfig {
            components: vec![DevComponent {
                name: "  ".to_string(),
                file: "comp.wasm".into(),
            }],
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("dev.components"));
    }

    #[test]
    fn dev_component_empty_file_is_err() {
        let cfg = DevConfig {
            components: vec![DevComponent {
                name: "sidecar".to_string(),
                file: "".into(),
            }],
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("file is empty"));
    }

    #[test]
    fn dev_component_valid_is_ok() {
        let cfg = DevConfig {
            components: vec![DevComponent {
                name: "sidecar".to_string(),
                file: "sidecar.wasm".into(),
            }],
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn dev_multiple_errors_are_all_reported() {
        let cfg = DevConfig {
            address: Some("bad-addr".to_string()),
            tls_cert_path: Some("cert.pem".into()),
            wasi_keyvalue_redis_url: Some("http://localhost".to_string()),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("dev.address"), "missing address error");
        assert!(err.contains("tls_cert_path"), "missing tls error");
        assert!(
            err.contains("wasi_keyvalue_redis_url"),
            "missing redis error"
        );
    }
}
