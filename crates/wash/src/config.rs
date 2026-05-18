//! Contains the [Config] struct and related functions for managing
//! wash configuration, including loading, saving, and merging configurations
//! with explicit defaults.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use figment::{
    Figment,
    providers::{Env, Format, Json, Toml, Yaml},
};
use serde::{Deserialize, Serialize};
use tracing::info;
use wash_runtime::host::allowed_hosts::AllowedHost;
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

    /// Workload-level configuration that describes the component being developed
    /// (env vars, wasi:config values, outbound allowlist). Field shape mirrors
    /// `WorkloadDeployment.spec.template.spec.components[].localResources`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workload: Option<WorkloadConfig>,

    /// Named ConfigMap-equivalent sources referenced by `workload.environment.configFrom`.
    ///
    /// `BTreeMap` so iteration / serialization order is deterministic.
    #[serde(
        default,
        rename = "configs",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub config_sources: BTreeMap<String, ConfigSource>,

    /// Named Secret-equivalent sources referenced by `workload.environment.secretFrom`.
    ///
    /// `BTreeMap` so iteration / serialization order is deterministic.
    #[serde(
        default,
        rename = "secrets",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub secret_sources: BTreeMap<String, SecretSource>,

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
            workload: None,
            config_sources: BTreeMap::new(),
            secret_sources: BTreeMap::new(),
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

/// Serde default for [`WorkloadConfig::allowed_hosts`]: a single
/// [`AllowedHost::Any`] entry (allow-all). Fires only when the YAML
/// omits `allowedHosts` entirely — an explicit `allowedHosts: []` stays
/// empty (deny-all in the runtime).
fn default_allow_all_hosts() -> Vec<AllowedHost> {
    vec![AllowedHost::Any]
}

/// Workload-level configuration that mirrors the `localResources` shape of a
/// `WorkloadDeployment` component.
///
/// Currently consumed by `wash dev`; the same shape is intended to round-trip
/// to a Kubernetes `WorkloadDeployment`.
///
/// Use [`WorkloadConfig::builder`] to construct so future fields don't break
/// callers.
#[derive(Debug, Clone, Default, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WorkloadConfig {
    /// Environment variables for the component (wasi:cli/env). Combines inline
    /// values with named references to top-level `configs:` and `secrets:`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentLayer>,
    /// Opaque key-value config delivered to the component (e.g. wasi:config/store).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub config: HashMap<String, String>,
    /// Outbound HTTP allowlist. Each entry parses into a typed
    /// [`AllowedHost`]; YAML/JSON callers continue to write plain strings.
    ///
    /// Default resolution distinguishes "field omitted" from "explicit
    /// empty":
    ///
    /// - **Missing from YAML** → serde default fires →
    ///   `[AllowedHost::Any]` (allow-all). Keeps `wash dev` ergonomic for
    ///   users who haven't thought about egress.
    /// - **`allowedHosts: []` in YAML** → empty `Vec` is preserved.
    ///   `resolve_workload` passes it through unchanged; the runtime
    ///   (`wash-runtime::host::http::check_allowed_hosts`) treats empty
    ///   as deny-all. Explicit user intent is respected.
    /// - **`WorkloadConfig::default()` (Rust API)** → empty `Vec`
    ///   (derived `Default`), which the runtime treats as deny-all
    ///   — fail-closed for programmatic construction.
    ///
    /// The serialization side does NOT skip empty lists, so a round-trip
    /// preserves the explicit-empty intent.
    #[serde(default = "default_allow_all_hosts")]
    pub allowed_hosts: Vec<AllowedHost>,
}

/// One layer of environment variables.
///
/// Inline values are written directly; `configFrom` / `secretFrom` reference
/// named entries in the top-level `configs:` / `secrets:` blocks by name. On
/// key conflicts later entries win, in order: inline → configFrom → secretFrom
/// (matches K8s `envFrom` semantics).
#[derive(Debug, Clone, Default, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct EnvironmentLayer {
    /// Inline plain values. Suitable for non-sensitive defaults.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub config: HashMap<String, String>,
    /// Names of entries in the top-level `configs:` block.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_from: Vec<String>,
    /// Names of entries in the top-level `secrets:` block.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_from: Vec<String>,
}

/// A source of non-sensitive key-value pairs for a `configs:` entry.
///
/// Multiple fields can be set on a single entry. They merge last-wins in the
/// order `inline` → `file` → `fromEnv` (matches K8s ConfigMap merge
/// semantics). Resolution lives in [`crate::workload`] as
/// [`ConfigSource::resolve`].
///
/// See [`SecretSource`] for the sibling type that carries the stricter
/// posture (file-mode check, in-repo-tree warning, etc.). The two share
/// today's wire schema but are deliberately distinct types so secret
/// handling can never be applied to a config and vice versa.
#[derive(Debug, Clone, Default, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ConfigSource {
    /// Literal key-value entries supplied inline.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub inline: HashMap<String, String>,
    /// Path to a `.env`-format file. Relative paths resolve against the
    /// project directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// Names of environment variables to pull from the developer's shell.
    /// Each name is read at resolve time via [`std::env::var`]; a missing
    /// variable is a hard error.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from_env: Vec<String>,
}

/// A source of sensitive key-value pairs for a `secrets:` entry.
///
/// Same wire shape as [`ConfigSource`] today, but a distinct Rust type so
/// the stricter resolve-time posture (Unix file mode `0600`/`0400`,
/// `O_NOFOLLOW` open + `fstat` perm check, in-repo-tree warning, no value
/// snippets in error / log output) can only be applied here. Resolution
/// lives in [`crate::workload`] as [`SecretSource::resolve`].
///
/// The two types may diverge in the future (e.g. a future `rotation`
/// field that only makes sense for secrets) — keeping them separate now
/// avoids retrofitting the type split later.
#[derive(Debug, Clone, Default, Serialize, Deserialize, bon::Builder)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SecretSource {
    /// Literal key-value entries supplied inline. Convenient for dev /
    /// test; do not commit production secrets this way.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub inline: HashMap<String, String>,
    /// Path to a `.env`-format file. Relative paths resolve against the
    /// project directory. The file must be Unix mode `0600` or `0400`
    /// and must not escape the project directory via `..` or symlink.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// Names of environment variables to pull from the developer's shell.
    /// Each name is read at resolve time via [`std::env::var`]; a missing
    /// variable is a hard error.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from_env: Vec<String>,
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
    /// Expected path to the built Wasm component artifact for dev mode.
    /// Overrides `build.component_path`. Useful when `dev.command` builds a
    /// different artifact (e.g. cargo debug profile in `target/.../debug/`
    /// instead of `release/`). Relative paths are resolved against the project
    /// directory. Exposed to build commands via `WASH_COMPONENT_PATH`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_path: Option<PathBuf>,
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

    /// Environment variables exported into the `wash dev` process before the
    /// host is built. Surfaces values to plugins and runtime crates that read
    /// from `std::env` (e.g. `RUST_LOG`, `OTEL_*`, libpq's `PG*` family).
    /// Distinct from `workload.environment`, which is delivered to the
    /// component via `wasi:cli/env`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub environment: HashMap<String, String>,

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

/// Export `dev.environment` from the loaded wash config into the current
/// process via `std::env::set_var`. Must be called from `main()` *before*
/// plugins, so that values like `OTEL_*` and `RUST_LOG`
/// configured under `dev.environment` are visible to the tracing subscriber
/// (which reads `OTEL_*` and `RUST_LOG` at init time and never again).
///
/// Best-effort: if the global XDG config dir can't be determined or the
/// project config can't be loaded, returns silently. The tracing
/// subscriber isn't initialized at this point so we have nowhere to log.
///
/// # Safety
///
/// `std::env::set_var` is `unsafe` in the 2024 edition because it races
/// with concurrent `getenv` from other threads. Callers MUST invoke this
/// once, very early in `main()`, before any worker thread has begun
/// reading env vars.
#[allow(unsafe_code)]
pub fn apply_dev_environment(user_config_override: Option<&Path>, project_dir: &Path) {
    let global_config_path = match user_config_override {
        Some(path) => path.to_path_buf(),
        None => {
            let Ok(strategy) = etcetera::choose_app_strategy(etcetera::AppStrategyArgs {
                top_level_domain: "com.wasmcloud".to_string(),
                author: "wasmCloud Team".to_string(),
                app_name: "wash".to_string(),
            }) else {
                return;
            };
            locate_user_config(&etcetera::AppStrategy::config_dir(&strategy))
        }
    };

    let Ok(config) = load_config::<Config>(&global_config_path, Some(project_dir), None) else {
        return;
    };

    for (key, value) in &config.dev().environment {
        // SAFETY: see function-level docs.
        unsafe { std::env::set_var(key, value) };
    }
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
        workload: None,
        config_sources: BTreeMap::new(),
        secret_sources: BTreeMap::new(),
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
    fn dev_environment_deserializes_from_yaml() {
        // Locks in the YAML contract surfaced to users:
        //
        //   dev:
        //     environment:
        //       KEY: value
        //
        // A regression here (e.g. someone adding `rename_all = "camelCase"`
        // to `DevConfig`, or moving the field) would silently drop user-
        // configured env vars at `wash dev` startup.
        let yaml = r#"
dev:
  environment:
    RUST_LOG: debug
    OTEL_EXPORTER_OTLP_ENDPOINT: http://localhost:4317
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let env = config.dev().environment;
        assert_eq!(env.get("RUST_LOG").map(String::as_str), Some("debug"));
        assert_eq!(
            env.get("OTEL_EXPORTER_OTLP_ENDPOINT").map(String::as_str),
            Some("http://localhost:4317")
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
    fn workload_yaml_uses_camel_case_for_renamed_fields() {
        // `WorkloadConfig`, `EnvironmentLayer`, and `ConfigSource` carry
        // `rename_all = "camelCase"`. Users write `configFrom` / `secretFrom`
        // / `allowedHosts` / `fromEnv` in YAML; if a refactor drops one of
        // those `rename_all` attributes, the camelCase keys get silently
        // dropped (parses fine, fields stay default). Pin the contract.
        let yaml = r#"
workload:
  environment:
    config:
      INLINE_KEY: inline_value
    configFrom:
      - app
    secretFrom:
      - creds
  config:
    flag: "on"
  allowedHosts:
    - https://api.example.com
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let workload = config.workload.expect("workload should parse");

        let env = workload
            .environment
            .expect("environment layer should parse");
        assert_eq!(env.config.get("INLINE_KEY").unwrap(), "inline_value");
        assert_eq!(env.config_from, vec!["app".to_string()]);
        assert_eq!(env.secret_from, vec!["creds".to_string()]);

        assert_eq!(workload.config.get("flag").unwrap(), "on");
        assert_eq!(
            workload.allowed_hosts,
            vec!["https://api.example.com".parse().unwrap()]
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

    #[test]
    fn configs_and_secrets_named_map_with_camel_case_source_fields() {
        // The top-level `configs:` and `secrets:` blocks are name -> ConfigSource
        // maps, and ConfigSource's `from_env` field is `fromEnv` in YAML.
        // `secrets:` shares the same struct as `configs:` — pin both so a
        // future split into separate types doesn't silently lose schema parity.
        let yaml = r#"
configs:
  app:
    inline:
      APP_FOO: app_foo_value
    file: ./app.env
secrets:
  creds:
    fromEnv:
      - DB_PASSWORD
    inline:
      DB_USER: alice
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();

        let app = config
            .config_sources
            .get("app")
            .expect("configs.app should parse");
        assert_eq!(app.inline.get("APP_FOO").unwrap(), "app_foo_value");
        assert_eq!(app.file.as_deref(), Some(Path::new("./app.env")));

        let creds = config
            .secret_sources
            .get("creds")
            .expect("secrets.creds should parse");
        assert_eq!(creds.from_env, vec!["DB_PASSWORD".to_string()]);
        assert_eq!(creds.inline.get("DB_USER").unwrap(), "alice");
    }

    #[test]
    fn dev_environment_defaults_to_empty() {
        // `dev.environment` is optional — a `dev:` block without it must
        // not fail to parse, and must produce an empty map (not panic on
        // the `set_var` loop reading a None).
        let yaml = "dev: {}\n";
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(config.dev().environment.is_empty());
    }
}
