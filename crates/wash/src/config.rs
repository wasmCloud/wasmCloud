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
use wash_runtime::component_source::ComponentSource;
use wash_runtime::host::allowed_hosts::AllowedHost;
use wash_runtime::host::allowed_ip_name::AllowedIpName;
use wash_runtime::host::allowed_loopback::AllowedLoopbackPort;
use wash_runtime::oci::OciPullPolicy;
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

    /// `wash host` configuration (default: empty/optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<HostConfig>,

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
            host: None,
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

    /// Get the `wash host` configuration, defaulting to [HostConfig::default()] if not set
    pub fn host(&self) -> HostConfig {
        self.host.clone().unwrap_or_default()
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
    /// Names components may resolve through
    /// `wasi:sockets/ip-name-lookup` (`resolve-addresses`). Each entry
    /// parses into a typed [`AllowedIpName`]; YAML/JSON callers write plain
    /// strings such as `"*"`, `"*.example.com"`, `"example.com"`, or a
    /// literal IP address.
    ///
    /// An omitted or empty list denies every lookup. Resolution is opt-in:
    /// nothing substitutes an allow-all policy for a workload that
    /// declared none.
    #[serde(default)]
    #[builder(default)]
    pub allowed_ip_name_lookups: Vec<AllowedIpName>,
    /// Ports on the machine's own loopback components may reach through
    /// `host.wasmcloud.internal`. Each entry is a port with an optional
    /// protocol: `5432`, `5432/tcp`, `53/udp`.
    ///
    /// An omitted or empty list denies every host-loopback connection, and a
    /// non-empty one is inert unless the host runs with
    /// `--allow-host-loopback`. `127.0.0.1` keeps meaning the workload's own
    /// virtual network either way.
    #[serde(default)]
    #[builder(default)]
    pub allowed_host_loopback_ports: Vec<AllowedLoopbackPort>,
}

// The `configs:`/`secrets:` source model moved to wash-runtime so every
// embedder resolves these the same way (see
// `wash_runtime::config_source`). Re-exported here because this module is
// the documented home of the config schema.
pub use wash_runtime::config_source::{ConfigSource, EnvironmentLayer, SecretSource};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevVolume {
    /// Host path to mount
    pub host_path: PathBuf,
    /// Guest path inside the dev environment
    pub guest_path: PathBuf,
}

/// Where a config block's wasm component comes from: exactly one of `file` or
/// `image`, plus a pull policy that only an image can honor.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ComponentSourceConfig {
    /// Local wasm file path. Mutually exclusive with `image`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// OCI image reference. Mutually exclusive with `file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Pull policy for `image` sources: `always`, `ifNotPresent`, or `never`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_policy: Option<String>,
}

impl ComponentSourceConfig {
    /// A local file source.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            file: Some(path.into()),
            ..Default::default()
        }
    }

    /// An OCI image source, pulled under the default policy.
    pub fn image(image: impl Into<String>) -> Self {
        Self {
            image: Some(image.into()),
            ..Default::default()
        }
    }

    /// Resolve to a runtime [`ComponentSource`].
    ///
    /// `name` names the config block and leads every error. Pass something the
    /// user can find in their `config.yaml`, e.g. `"dev.components['sidecar']"`.
    pub fn to_source(&self, name: &str) -> Result<ComponentSource> {
        let pull_policy = match &self.pull_policy {
            Some(policy) => Some(
                policy
                    .parse::<OciPullPolicy>()
                    .with_context(|| name.to_string())?,
            ),
            None => None,
        };
        ComponentSource::from_image_or_file(
            self.image.clone(),
            self.file.clone(),
            pull_policy,
            name,
        )
    }
}

/// A component loaded alongside the main dev component.
///
/// `environment` / `config` / `allowedHosts` / `allowedIpNameLookups` override
/// the workload-level `workload:` block for this component. See
/// [`crate::workload::resolve_component_workload`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevComponent {
    /// Name of the component
    pub name: String,
    /// Where the component's wasm comes from: a local `file` or an `image`.
    #[serde(flatten)]
    pub source: ComponentSourceConfig,
    /// Environment variables (wasi:cli/env), merged over
    /// `workload.environment`. This component wins on key conflicts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentLayer>,
    /// Opaque key-value config, merged over `workload.config`. This
    /// component wins on key conflicts.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub config: HashMap<String, String>,
    /// Outbound HTTP allowlist. When set it replaces `workload.allowedHosts`
    /// for this component (`[]` denies all egress); when omitted the
    /// workload list applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_hosts: Option<Vec<AllowedHost>>,
    /// Names this component may resolve through
    /// `wasi:sockets/ip-name-lookup`. When set it replaces
    /// `workload.allowedIpNameLookups` for this component (an explicit `[]`
    /// denies every lookup); when omitted the workload list applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_ip_name_lookups: Option<Vec<AllowedIpName>>,
    /// Host-loopback ports this component may reach. When set it replaces
    /// `workload.allowedHostLoopbackPorts` for this component; when omitted the
    /// workload list applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_host_loopback_ports: Option<Vec<AllowedLoopbackPort>>,
    /// How many instances of this component to keep warm between calls.
    ///
    /// Unset (or `0`) keeps the default: every call runs in a fresh instance
    /// and component state is ephemeral. Setting it lets an instance be reused
    /// by the next call, so whatever the guest caches in memory — a connection
    /// pool, a lazily built runtime — survives instead of being rebuilt per
    /// call. Work past what the warm instances can take is still served, from
    /// fresh ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_size: Option<i32>,
    /// How many calls a warm instance serves before it is retired and the next
    /// call starts cold. Unset (or `0`) means no limit. Only meaningful
    /// alongside `poolSize`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_invocations: Option<i32>,
    /// How many calls one warm instance may serve at the same time.
    ///
    /// Unset means one, which is what a component gets without asking: an
    /// instance serves a single call at a time. Raising it lets an instance
    /// overlap calls while it is awaiting I/O, which is where a pool of
    /// instances would otherwise sit idle.
    ///
    /// Only safe for a guest that yields rather than blocks. A guest driving
    /// its own executor — anything calling `block_on` — must stay at one, or a
    /// second concurrent call will try to enter that executor from inside
    /// itself. Only meaningful alongside `poolSize`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<i32>,
}

impl DevComponent {
    /// Creates a file-backed component entry with no per-component overrides.
    pub fn new(name: impl Into<String>, file: impl Into<PathBuf>) -> Self {
        Self::from_source(name, ComponentSourceConfig::file(file))
    }

    /// Creates a component entry from any source with no per-component
    /// overrides.
    pub fn from_source(name: impl Into<String>, source: ComponentSourceConfig) -> Self {
        Self {
            name: name.into(),
            source,
            environment: None,
            config: HashMap::new(),
            allowed_hosts: None,
            allowed_ip_name_lookups: None,
            allowed_host_loopback_ports: None,
            pool_size: None,
            max_invocations: None,
            max_concurrency: None,
        }
    }
}

/// A host component plugin to load into the `wash dev` host: a WebAssembly
/// component that provides a host capability, served to every workload that
/// imports its interface. Provide exactly one of `file` (local path) or `image`
/// (OCI reference). Requires a wash build with the `host-component-plugins`
/// feature.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPluginConfig {
    /// Host-unique plugin id.
    pub id: String,
    /// Where the plugin's wasm comes from: a local `file` or an `image`.
    #[serde(flatten)]
    pub source: ComponentSourceConfig,
    /// Supervised driver restarts before the plugin is declared dead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_restarts: Option<u32>,
    /// OCI digest to pin (`image` sources only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_digest: Option<String>,
    /// This plugin's own bind-time config, delivered to every native
    /// capability it imports (e.g. `wasmcloud:secrets`) via `on-workload-bind`
    /// — never written to a file the plugin itself reads. `config`/
    /// `configFrom`/`secretFrom` merge exactly like `workload.environment`
    /// (inline → configFrom → secretFrom, last source wins), resolved
    /// against the same top-level `configs:`/`secrets:` catalogs.
    #[serde(flatten)]
    pub environment: EnvironmentLayer,
    /// Hosts this plugin's own `wasi:http/outgoing-handler` calls may reach.
    /// Unlike a workload's `allowedHosts`, an omitted list denies every
    /// outbound host by default — a host component plugin is
    /// operator-controlled, more privileged than a workload, and gets no
    /// ergonomic allow-all default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<AllowedHost>,
    /// Names this plugin's own `wasi:sockets/ip-name-lookup` calls may
    /// resolve. An omitted or empty list denies every lookup.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_ip_name_lookups: Vec<AllowedIpName>,
    /// Ports this plugin listens on. An omitted or empty list means it binds
    /// nothing reachable, which is what every plugin got before ports existed.
    ///
    /// Each entry needs a `name` and the `port` the plugin's own code binds.
    /// Optional: `protocol` (TCP or UDP, default TCP) and exactly one of
    ///
    ///   publish   real port the host binds, splicing accepted connections
    ///             into the plugin's private virtual loopback. The plugin
    ///             binds `127.0.0.1:<port>` and needs no change.
    ///   bind      concrete address the plugin binds itself, skipping the
    ///             splice. Rejected if unspecified (`0.0.0.0`) or loopback.
    ///
    /// Neither declares the port without exposing it. Requires the host to be
    /// started with `--publish-ports`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<wash_runtime::host::declared_port::DeclaredPort>,
}

impl HostPluginConfig {
    /// Convert to a runtime [`wash_runtime::plugin::ComponentPluginSpec`],
    /// without resolving `configFrom`/`secretFrom` — used where no [`Config`]
    /// is available. Prefer [`HostPluginConfig::to_spec`] when one is.
    ///
    /// `expectedDigest` on a file source is caught by the loader when it finds
    /// no digest to check against, so this only has to validate what it can see
    /// without fetching.
    pub fn to_spec_unresolved(&self) -> Result<wash_runtime::plugin::ComponentPluginSpec> {
        if self.id.is_empty() {
            bail!("host_plugins entry is missing a non-empty `id`");
        }
        let what = format!("host_plugins '{}'", self.id);
        // Catch a bad port declaration here, where the error can name the
        // config entry, rather than at plugin start.
        wash_runtime::host::declared_port::validate_ports(&self.ports, &what)?;
        Ok(wash_runtime::plugin::ComponentPluginSpec {
            id: self.id.clone(),
            source: self.source.to_source(&what)?,
            max_restarts: self.max_restarts,
            expected_digest: self.expected_digest.clone(),
            config: self.environment.config.clone(),
            allowed_hosts: self.allowed_hosts.clone().into(),
            allowed_ip_name_lookups: self.allowed_ip_name_lookups.clone().into(),
            ports: self.ports.clone().into(),
        })
    }

    /// Convert to a runtime [`wash_runtime::plugin::ComponentPluginSpec`],
    /// resolving `configFrom`/`secretFrom` against `config`'s top-level
    /// `configs:`/`secrets:` catalogs the same way a workload's
    /// `environment.configFrom`/`secretFrom` resolve.
    ///
    /// # Errors
    ///
    /// Same failure modes as [`crate::workload::resolve_workload`], for this
    /// plugin's own `configFrom`/`secretFrom` references.
    pub fn to_spec(
        &self,
        config: &Config,
        project_dir: &Path,
        repo_root: Option<&Path>,
    ) -> Result<wash_runtime::plugin::ComponentPluginSpec> {
        let mut spec = self.to_spec_unresolved()?;
        let owner = format!("host_plugins '{}'", self.id);
        spec.config = wash_runtime::config_source::resolve_environment_layer(
            Some(&self.environment),
            &owner,
            &config.config_sources,
            &config.secret_sources,
            project_dir,
            repo_root,
        )?;
        Ok(spec)
    }
}

/// `wash host` configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostConfig {
    /// Host component plugins to load: WebAssembly components that provide
    /// host capabilities. Requires a wash build with the
    /// `host-component-plugins` feature. Merges with (does not replace) any
    /// plugins declared via repeated `--host-plugin` flags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_plugins: Vec<HostPluginConfig>,
}

/// Built-in trust roots for outbound HTTPS from components, before any extra
/// CA bundles are layered on top. CLI/config mirror of
/// [`wash_runtime::host::http_client::TrustRoots`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum HttpClientTrustRoots {
    /// Compiled-in webpki roots plus the platform's native store.
    /// The native store honours `SSL_CERT_FILE`/`SSL_CERT_DIR`.
    WebpkiAndNative,
    /// Compiled-in webpki roots only — reproducible, ignores the host
    /// environment. The default, matching the behavior before this option
    /// existed.
    #[default]
    Webpki,
    /// Platform native store only.
    Native,
    /// No built-in roots: trust exactly the configured extra CA bundles.
    ExtraOnly,
}

impl HttpClientTrustRoots {
    // serde's `skip_serializing_if` hands the field by reference.
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl From<HttpClientTrustRoots> for wash_runtime::host::http_client::TrustRoots {
    fn from(roots: HttpClientTrustRoots) -> Self {
        match roots {
            HttpClientTrustRoots::WebpkiAndNative => Self::WebpkiAndNative,
            HttpClientTrustRoots::Webpki => Self::Webpki,
            HttpClientTrustRoots::Native => Self::Native,
            HttpClientTrustRoots::ExtraOnly => Self::ExtraOnly,
        }
    }
}

/// OTLP wire protocol for a `wasi:otel` target. CLI/config mirror of
/// [`wash_runtime::plugin::wasi_otel::OtelProtocol`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum OtelProtocol {
    /// OTLP over gRPC.
    #[serde(rename = "grpc")]
    #[value(name = "grpc")]
    Grpc,
    /// OTLP over HTTP with protobuf-encoded bodies.
    /// TODO: add support for http config. Should decide for an http lib to use first.
    #[serde(rename = "http/protobuf", alias = "http-protobuf")]
    #[value(name = "http/protobuf", alias = "http-protobuf")]
    HttpProtobuf,
}

impl From<OtelProtocol> for wash_runtime::plugin::wasi_otel::OtelProtocol {
    fn from(protocol: OtelProtocol) -> Self {
        match protocol {
            OtelProtocol::Grpc => Self::Grpc,
            OtelProtocol::HttpProtobuf => Self::HttpProtobuf,
        }
    }
}

/// Builds a [`wash_runtime::plugin::wasi_otel::WasiOtelConfig`] from the four
/// optional CLI/config host_endpoint, workload_endpoint, host_protocol, workload_protocol.
pub fn wasi_otel_config(
    host_endpoint: Option<&str>,
    host_protocol: Option<OtelProtocol>,
    workload_endpoint: Option<&str>,
    workload_protocol: Option<OtelProtocol>,
) -> wash_runtime::plugin::wasi_otel::WasiOtelConfig {
    use wash_runtime::plugin::wasi_otel::{OtelTarget, WasiOtelConfig};

    let target = |endpoint: Option<&str>, protocol: Option<OtelProtocol>| {
        endpoint.map(|e| {
            OtelTarget::builder()
                .endpoint(e)
                .maybe_protocol(protocol.map(Into::into))
                .build()
        })
    };

    WasiOtelConfig::builder()
        .maybe_host(target(host_endpoint, host_protocol))
        .maybe_workload(target(workload_endpoint, workload_protocol))
        .build()
}

/// Build the host's [`QuotaRegistry`] from optional config/CLI overrides.
///
/// One registry governs every surface a guest can hold a connection on, so
/// these are the numbers an operator tunes. `None` keeps the built-in default
/// for that setting.
///
/// # Errors
///
/// Rejects a zero for any ceiling or for the wait, which would silently mean
/// "no connections" or "never wait" rather than what the operator meant.
///
/// [`QuotaRegistry`]: wash_runtime::host::quota::QuotaRegistry
pub fn connection_quotas(
    max_connections: Option<usize>,
    max_http_per_workload: Option<usize>,
    max_sockets_per_workload: Option<usize>,
    max_inbound_per_workload: Option<usize>,
    http_connection_wait: Option<std::time::Duration>,
) -> anyhow::Result<std::sync::Arc<wash_runtime::host::quota::QuotaRegistry>> {
    let defaults = wash_runtime::host::quota::QuotaLimits::default();
    let resolve = |value: Option<usize>, default: usize, name: &str| -> anyhow::Result<usize> {
        match value {
            Some(0) => anyhow::bail!("{name} must be at least 1"),
            Some(v) => Ok(v),
            None => Ok(default),
        }
    };
    let limits = wash_runtime::host::quota::QuotaLimits {
        outbound_http: resolve(
            max_http_per_workload,
            defaults.outbound_http,
            "max_outbound_http_connections_per_workload",
        )?,
        outbound_sockets: resolve(
            max_sockets_per_workload,
            defaults.outbound_sockets,
            "max_outbound_socket_connections_per_workload",
        )?,
        inbound_sockets: resolve(
            max_inbound_per_workload,
            defaults.inbound_sockets,
            "max_inbound_socket_connections_per_workload",
        )?,
    };
    if max_connections == Some(0) {
        anyhow::bail!("max_connections must be at least 1");
    }
    if let Some(total) = max_connections
        && limits
            .outbound_http
            .max(limits.outbound_sockets)
            .max(limits.inbound_sockets)
            > total
    {
        // Harmless (the host-wide ceiling simply gates first), but almost
        // certainly an operator mixing the two knobs up.
        tracing::warn!(
            ?limits,
            max_connections = total,
            "a per-workload ceiling exceeds max_connections; the host-wide cap will gate first"
        );
    }

    // An unset flag means the built-in ceiling, not "unbounded": without one, a
    // crowd of workloads each holding its per-guest allowance exhausts the
    // host's file descriptors, and the failures land on ingress and OCI pulls
    // rather than on whoever caused them.
    let host_wide =
        max_connections.or_else(|| Some(wash_runtime::host::quota::default_max_connections()));
    let registry = wash_runtime::host::quota::QuotaRegistry::new(limits, host_wide);
    match http_connection_wait {
        Some(wait) if wait.is_zero() => {
            anyhow::bail!("http_connection_wait must be greater than zero")
        }
        Some(wait) => Ok(std::sync::Arc::new(
            registry.as_ref().clone().with_http_wait(wait),
        )),
        None => Ok(registry),
    }
}

/// Resolve the messaging admission ceilings into the [`MessagingLimits`] every
/// messaging backend on this host shares.
///
/// Mirrors [`connection_quotas`]: the same two-level host-wide/per-workload
/// shape, and the same treatment of a zero.
///
/// `None` for either ceiling means the operator said nothing, and the number is
/// derived from `total_core_instances` — the engine's actual pool budget — so a
/// host told it is larger sizes its messaging ceiling to match instead of
/// leaving a stock default silently binding. This is why the flags must not
/// carry a `default_value_t`: a parse-time default is indistinguishable here
/// from an operator typing the same number.
///
/// # Errors
///
/// Rejects a ceiling outside `1..=`[`MessagingLimits::MAX_IN_FLIGHT`]. Zero
/// would silently mean "process no messages", which is never what an operator
/// meant; a value above the maximum would panic inside the semaphore at
/// startup. Better a startup error than a host that looks healthy and quietly
/// consumes nothing, or one that aborts with a backtrace.
///
/// [`MessagingLimits`]: wash_runtime::plugin::wasmcloud_messaging::MessagingLimits
/// [`MessagingLimits::MAX_IN_FLIGHT`]: wash_runtime::plugin::wasmcloud_messaging::MessagingLimits::MAX_IN_FLIGHT
pub fn wasmcloud_messaging_limits(
    max_in_flight: Option<usize>,
    max_in_flight_per_component: Option<usize>,
    total_core_instances: Option<u32>,
) -> anyhow::Result<wash_runtime::plugin::wasmcloud_messaging::MessagingLimits> {
    use wash_runtime::plugin::wasmcloud_messaging::MessagingLimits;

    let checked = |value: Option<usize>, flag: &str| -> anyhow::Result<Option<usize>> {
        match value {
            Some(0) => anyhow::bail!("{flag} must be at least 1"),
            Some(v) if v > MessagingLimits::MAX_IN_FLIGHT => anyhow::bail!(
                "{flag} must be at most {} (the most a semaphore can hold), got {v}",
                MessagingLimits::MAX_IN_FLIGHT
            ),
            other => Ok(other),
        }
    };

    // A per-component ceiling above the host-wide total is harmless — the host
    // semaphore gates first — but almost certainly an operator mixing the two
    // knobs up, so `MessagingLimits::new` warns about it, exactly as
    // `connection_quotas` does for its equivalent.
    let limits = MessagingLimits::resolve(
        checked(max_in_flight, "wasmcloud_messaging_max_in_flight")?,
        checked(
            max_in_flight_per_component,
            "wasmcloud_messaging_max_in_flight_per_component",
        )?,
        total_core_instances,
    );

    // Both numbers vary by host — pooling on or off, the size of the pool, and
    // which flags were given — so an operator cannot read them off the docs.
    // They are also the numbers a shed warning tells them to go and raise, which
    // makes this the one derived ceiling worth a line at startup.
    tracing::info!(
        host_total = limits.host_total(),
        per_component_default = limits.per_component_default(),
        host_total_source = if max_in_flight.is_some() {
            "flag"
        } else if total_core_instances.is_some() {
            "derived from the instance pool"
        } else {
            "built-in default (pooling disabled)"
        },
        per_component_source = if max_in_flight_per_component.is_some() {
            "flag"
        } else {
            "derived from the host total"
        },
        "wasmcloud:messaging admission ceilings resolved"
    );

    Ok(limits)
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
    /// Optional path to a wasm component to be used as a service. Mutually
    /// exclusive with `service_image`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_file: Option<PathBuf>,
    /// Optional OCI image for the component to be used as a service. Mutually
    /// exclusive with `service_file`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_image: Option<String>,
    /// Pull policy for `service_image`: `always`, `ifNotPresent`, or `never`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_pull_policy: Option<String>,
    /// Ports the service listens on inside the workload's virtual loopback,
    /// and which of them the host exposes on a real address.
    ///
    /// Each entry needs a `name` and the `port` the service binds on
    /// `127.0.0.1`; add `publish: <hostPort>` to expose it. Omitting `publish`
    /// declares the port without exposing it, which is exactly today's
    /// behavior. `bind` is not accepted here — handing a guest a real listening
    /// socket is an operator's call, and a workload's ports are not.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub service_ports: Vec<wash_runtime::host::declared_port::DeclaredPort>,

    /// Reach registries over HTTP instead of HTTPS. Applies to every image a
    /// dev session pulls components, the service, and host plugins.
    /// Mirrors `wash host --allow-insecure-registries`.
    #[serde(default)]
    pub allow_insecure_registries: bool,

    /// Additional components to load alongside the main component
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub components: Vec<DevComponent>,

    /// Host component plugins to load into the dev host: WebAssembly components
    /// that provide host capabilities. Requires a wash build with the
    /// `host-component-plugins` feature.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_plugins: Vec<HostPluginConfig>,

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

    /// Extra CA certificate bundle files (PEM) trusted for *outbound* HTTPS
    /// requests made by the component (`wasi:http` outgoing handler), layered
    /// on top of `http_client_trust_roots`. Use this to reach hosts behind a
    /// corporate or otherwise private CA. Unlike `tls_ca_path` (which
    /// configures the ingress HTTP server), these apply to requests the
    /// component sends out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub http_client_ca_paths: Vec<PathBuf>,

    /// Built-in trust roots for the component's *outbound* HTTPS requests,
    /// before `http_client_ca_paths` bundles are layered on top. Defaults to
    /// `webpki`; set `webpki-and-native` to also trust the platform store
    /// (which honours `SSL_CERT_FILE`/`SSL_CERT_DIR`).
    #[serde(default, skip_serializing_if = "HttpClientTrustRoots::is_default")]
    pub http_client_trust_roots: HttpClientTrustRoots,

    /// Raw `wasi:sockets` connections one workload may hold.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_outbound_socket_connections_per_workload: Option<usize>,

    /// Inbound published-port connections one workload may serve at once.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_inbound_socket_connections_per_workload: Option<usize>,

    /// Host-wide cap on live *outbound* HTTP connections across all
    /// workloads combined (in-flight or idle in a keep-alive pool). Defaults
    /// to the runtime's built-in limit; size it for the number of
    /// concurrently busy workloads times their burst concurrency.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_connections: Option<usize>,

    /// Cap on live *outbound* HTTP connections a single workload may hold,
    /// across all authorities it talks to. Defaults to the runtime's
    /// built-in limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_outbound_http_connections_per_workload: Option<usize>,

    /// How long an outbound request waits for a connection slot once one of
    /// the caps above is reached, before failing with a connect timeout.
    /// A humantime duration such as `5s` or `500ms`; defaults to the
    /// runtime's built-in wait. A component's own `connect-timeout` bounds
    /// its request independently, so this only decides how long an attempt
    /// nothing is waiting on may hold a slot reservation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_connection_wait: Option<String>,

    /// Enable WASI WebGPU support in the dev environment. Only supported on non-Windows platforms.
    #[serde(default)]
    pub wasi_webgpu: bool,

    /// Shared NATS connection URL for the data-plane plugins
    /// blobstore, keyvalue, and messaging. Mirroring the `wash host`
    /// with `--data-nats-url`. When set, all three use NATS unless a per-plugin
    /// URL below overrides it (e.g. `wasi_keyvalue_redis_url`,
    /// `wasi_keyvalue_path`, `wasi_keyvalue_nats_url`, `wasi_blobstore_path`).
    /// Example: nats://127.0.0.1:4222
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_nats_url: Option<String>,

    /// Optional Redis connection URL for the WASI keyvalue plugin.
    /// Example: redis://127.0.0.1:6379
    /// When set, takes precedence over wasi_keyvalue_path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasi_keyvalue_redis_url: Option<String>,

    /// Optional path for WASI keyvalue filesystem storage. If not set, an in-memory store is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasi_keyvalue_path: Option<PathBuf>,

    /// Optional NATS connection URL for the WASI keyvalue plugin. Overrides
    /// `data_nats_url` for keyvalue.
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

    /// Optional OTEL collector endpoint for host/platform Spans, Logs, Metrics.
    /// If not set, falls back to OTEL_EXPORTER_OTLP_TRACES_ENDPOINT or OTEL_EXPORTER_OTLP_ENDPOINT env var.
    /// Example: http://platform-collector:4317
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_host_endpoint: Option<String>,

    /// Optional OTEL collector endpoint for workload/application Spans, Logs, Metrics.
    /// If not set, falls back to otel_host_endpoint.
    /// Example: http://app-collector:4317
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_workload_endpoint: Option<String>,

    /// OTLP protocol for host/platform telemetry: `grpc` or `http/protobuf`.
    /// If not set, falls back to OTEL_EXPORTER_OTLP_PROTOCOL env var, then `grpc`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_host_protocol: Option<OtelProtocol>,

    /// OTLP protocol for workload/application telemetry: `grpc` or `http/protobuf`.
    /// If not set, falls back to otel_host_protocol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub otel_workload_protocol: Option<OtelProtocol>,

    /// Additional wasm proposals to enable on the engine, by name. Accepted
    /// names match `wash_runtime`'s `WasmProposal`: component-model-async,
    /// component-model-map, gc, exception-handling, wide-arithmetic, threads,
    /// tail-call.
    #[serde(default)]
    pub wasm_proposals: Vec<String>,
}

impl DevConfig {
    /// The connection quota registry this dev config asks for.
    ///
    /// Lives here rather than at the call site so the five knobs are read in
    /// one place, next to the fields they come from — adding a surface means
    /// touching this method, not every caller.
    ///
    /// # Errors
    ///
    /// Fails if `http_connection_wait` is not a duration, or if any ceiling is
    /// zero.
    pub fn connection_quotas(
        &self,
    ) -> Result<std::sync::Arc<wash_runtime::host::quota::QuotaRegistry>> {
        let http_connection_wait = self
            .http_connection_wait
            .as_deref()
            .map(humantime::parse_duration)
            .transpose()
            .context("dev.http_connection_wait is not a valid duration (e.g. `5s`)")?;
        connection_quotas(
            self.max_connections,
            self.max_outbound_http_connections_per_workload,
            self.max_outbound_socket_connections_per_workload,
            self.max_inbound_socket_connections_per_workload,
            http_connection_wait,
        )
    }

    /// Where the separately-configured service component comes from, or `None`
    /// when none is configured.
    ///
    /// `dev.service = true` makes the component under development the service,
    /// and then this is ignored. See `wash dev`'s workload assembly.
    pub fn service_source(&self) -> Result<Option<ComponentSource>> {
        if self.service_file.is_none() && self.service_image.is_none() {
            return Ok(None);
        }
        ComponentSourceConfig {
            file: self.service_file.clone(),
            image: self.service_image.clone(),
            pull_policy: self.service_pull_policy.clone(),
        }
        .to_source("dev.service_file/service_image")
        .map(Some)
    }

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

        if let Some(url) = &self.data_nats_url {
            check_url_scheme("dev.data_nats_url", url, &["nats", "tls"], &mut errors);
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
        if let Some(url) = &self.otel_host_endpoint {
            check_url_scheme(
                "dev.otel_host_endpoint",
                url,
                &["http", "https"],
                &mut errors,
            );
        }
        if let Some(url) = &self.otel_workload_endpoint {
            check_url_scheme(
                "dev.otel_workload_endpoint",
                url,
                &["http", "https"],
                &mut errors,
            );
        }

        if cfg!(target_os = "windows") && self.wasi_webgpu {
            errors.push("dev.wasi_webgpu is not supported on Windows".to_string());
        }
        if cfg!(target_arch = "s390x") && self.wasi_webgpu {
            errors.push("dev.wasi_webgpu is not supported on s390x".to_string());
        }

        for proposal in &self.wasm_proposals {
            if let Err(err) = proposal.parse::<wash_runtime::engine::WasmProposal>() {
                errors.push(format!("dev.wasm_proposals: {err}"));
            }
        }

        for comp in &self.components {
            if comp.name.trim().is_empty() {
                errors.push("dev.components contains an entry with empty name".to_string());
            }
            if let Err(err) = comp
                .source
                .to_source(&format!("dev.components['{}']", comp.name))
            {
                errors.push(format!("{err:#}"));
            }
        }

        for plugin in &self.host_plugins {
            if let Err(err) = plugin.to_spec_unresolved() {
                errors.push(format!("{err:#}"));
            }
        }

        if let Err(err) = self.service_source() {
            errors.push(format!("{err:#}"));
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
        .context("failed to load wash configuration")
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
            bail!("Unsupported global config file extension: {ext}");
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
        serde_yaml_ng::to_string(config).context("failed to serialize configuration")?;

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
            components: vec![DevComponent::new(
                "example-sidecar",
                "example/path/to/sidecar.wasm",
            )],
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
            data_nats_url: Some("nats://127.0.0.1:4222".to_string()),
            wasi_keyvalue_redis_url: Some("redis://127.0.0.1:6379".to_string()),
            wasi_keyvalue_path: Some(PathBuf::from("./data/keyvalue")),
            wasi_keyvalue_nats_url: Some("nats://127.0.0.1:4222".to_string()),
            wasi_blobstore_path: Some(PathBuf::from("./data/blobstore")),
            postgres_url: Some("postgres://user:pass@127.0.0.1:5432".to_string()),
            ..Default::default()
        }),
        host: None,
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
    use std::time::Duration;

    use super::*;

    #[test]
    fn build_no_command_is_ok() {
        assert!(BuildConfig::default().validate().is_ok());
    }

    #[test]
    fn connection_quotas_default_when_unset() {
        let quotas = connection_quotas(None, None, None, None, None).unwrap();
        let defaults = wash_runtime::host::quota::QuotaLimits::default();
        assert_eq!(quotas.limits(), defaults);
    }

    #[test]
    fn connection_quotas_apply_overrides() {
        let quotas = connection_quotas(
            Some(64),
            Some(8),
            Some(16),
            Some(32),
            Some(Duration::from_millis(250)),
        )
        .unwrap();
        assert_eq!(quotas.limits().outbound_http, 8);
        assert_eq!(quotas.limits().outbound_sockets, 16);
        assert_eq!(quotas.limits().inbound_sockets, 32);
        assert_eq!(quotas.http_wait(), Duration::from_millis(250));
    }

    /// Each surface is its own ceiling, so filling one must leave the others
    /// alone — the property the unified quota exists to provide.
    #[test]
    fn connection_quotas_are_per_surface_and_per_guest() {
        let quotas = connection_quotas(None, Some(4), Some(1), Some(1), None).unwrap();
        let guest = quotas.for_guest("w-1");
        let _held = guest
            .try_acquire_outbound_socket()
            .expect("its one socket slot");
        assert!(
            guest.try_acquire_outbound_socket().is_none(),
            "sockets are at ceiling"
        );
        assert!(
            guest.try_acquire_inbound_socket().is_some(),
            "inbound must not be affected"
        );
        assert_eq!(
            guest.outbound_http_available(),
            4,
            "http must not be affected"
        );
        assert!(
            quotas
                .for_guest("w-2")
                .try_acquire_outbound_socket()
                .is_some(),
            "another guest has its own allowance"
        );
    }

    /// Every knob is a hard bound, so a zero would wedge that surface
    /// entirely — reject it at startup rather than at the first request.
    #[test]
    fn connection_quotas_reject_zero() {
        assert!(connection_quotas(Some(0), None, None, None, None).is_err());
        assert!(connection_quotas(None, Some(0), None, None, None).is_err());
        assert!(connection_quotas(None, None, Some(0), None, None).is_err());
        assert!(connection_quotas(None, None, None, Some(0), None).is_err());
        assert!(connection_quotas(None, None, None, None, Some(Duration::ZERO)).is_err());
    }

    #[test]
    fn wasmcloud_messaging_limits_reject_zero() {
        // Zero would silently mean "process no messages" — a host that looks
        // healthy and quietly consumes nothing. Better a startup error.
        assert!(wasmcloud_messaging_limits(Some(0), Some(32), None).is_err());
        assert!(wasmcloud_messaging_limits(Some(128), Some(0), None).is_err());
    }

    #[test]
    fn wasmcloud_messaging_limits_reject_more_than_a_semaphore_can_hold() {
        // Above this the semaphore panics at startup, so an unchecked value is
        // not a large ceiling but an abort with a backtrace. `usize::MAX` is
        // what a fat-fingered "unlimited" looks like.
        let too_big = wash_runtime::plugin::wasmcloud_messaging::MessagingLimits::MAX_IN_FLIGHT + 1;
        assert!(wasmcloud_messaging_limits(Some(too_big), None, None).is_err());
        assert!(wasmcloud_messaging_limits(None, Some(too_big), None).is_err());
        assert!(wasmcloud_messaging_limits(Some(usize::MAX), None, None).is_err());
    }

    #[test]
    fn wasmcloud_messaging_limits_apply_overrides() {
        let limits = wasmcloud_messaging_limits(Some(64), Some(8), None).expect("valid ceilings");
        assert_eq!(limits.host_total(), 64);
        assert_eq!(limits.per_component_default(), 8);

        // An explicit ceiling wins over what the pool would have derived —
        // otherwise the flag would be advisory on a pooled host.
        let limits =
            wasmcloud_messaging_limits(Some(64), Some(8), Some(3000)).expect("valid ceilings");
        assert_eq!(limits.host_total(), 64);
        assert_eq!(limits.per_component_default(), 8);
    }

    #[test]
    fn wasmcloud_messaging_limits_default_to_the_documented_pair() {
        // No flags and no pool to derive from: the pinned defaults stand.
        let limits =
            wasmcloud_messaging_limits(None, None, None).expect("the built-in defaults are valid");
        assert_eq!(limits.host_total(), 128);
        assert_eq!(limits.per_component_default(), 32);
    }

    #[test]
    fn setting_only_the_host_ceiling_still_moves_the_per_component_default() {
        // The operator-visible symptom of deriving the two independently:
        // `--wasmcloud-messaging-max-in-flight 1024` was accepted, the host
        // ceiling rose, and every component stayed pinned at the pool-derived
        // default — so on any host with fewer than ~31 messaging components the
        // flag changed nothing at all.
        let derived = wasmcloud_messaging_limits(None, None, Some(1000)).expect("valid");
        let raised = wasmcloud_messaging_limits(Some(1024), None, Some(1000)).expect("valid");
        assert_eq!(raised.host_total(), 1024);
        assert!(
            raised.per_component_default() > derived.per_component_default(),
            "raising only the host ceiling left the per-component default at {}",
            raised.per_component_default()
        );

        // Lowering it must not leave the per-component default stranded above
        // the total the operator just set.
        let lowered = wasmcloud_messaging_limits(Some(4), None, Some(1000)).expect("valid");
        assert!(lowered.per_component_default() <= lowered.host_total());
    }

    #[test]
    fn wasmcloud_messaging_limits_do_not_over_commit_a_small_pool() {
        // A pool of 16 core instances holds 3 worst-case components. The
        // derived ceiling must not exceed that: admitting 8 would need 40 core
        // instances and fail at instantiation, which is the exhaustion these
        // ceilings exist to prevent.
        let limits = wasmcloud_messaging_limits(None, None, Some(16)).expect("valid");
        assert!(
            limits.host_total() <= 3,
            "a 16-instance pool derived a ceiling of {} messages",
            limits.host_total()
        );
    }

    #[test]
    fn wasmcloud_messaging_limits_scale_with_the_pool() {
        // The point of deriving: a host told it is larger gets a larger
        // messaging ceiling, instead of the stock default silently binding.
        let stock = wasmcloud_messaging_limits(None, None, Some(1000)).expect("valid");
        let big = wasmcloud_messaging_limits(None, None, Some(8000)).expect("valid");
        assert!(
            big.host_total() > stock.host_total(),
            "raising WASMTIME_POOLING_TOTAL_CORE_INSTANCES must raise the messaging ceiling: \
             {} vs {}",
            big.host_total(),
            stock.host_total()
        );
        // And the per-component ceiling never exceeds the host one, however the
        // pool is sized.
        for limits in [&stock, &big] {
            assert!(limits.per_component_default() <= limits.host_total());
        }
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
    fn dev_otel_endpoints_deserialize_from_yaml() {
        let yaml = r#"
dev:
  wasi_otel: true
  otel_host_endpoint: http://platform-collector:4317
  otel_workload_endpoint: http://app-collector:4317
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let dev = config.dev();
        assert!(dev.wasi_otel);
        assert_eq!(
            dev.otel_host_endpoint.as_deref(),
            Some("http://platform-collector:4317")
        );
        assert_eq!(
            dev.otel_workload_endpoint.as_deref(),
            Some("http://app-collector:4317")
        );
        assert!(dev.validate().is_ok());
    }

    #[test]
    fn dev_otel_host_endpoint_bad_scheme_is_err() {
        let cfg = DevConfig {
            otel_host_endpoint: Some("nats://localhost:4317".to_string()),
            ..Default::default()
        };
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("dev.otel_host_endpoint")
        );
    }

    #[test]
    fn dev_otel_workload_endpoint_bad_scheme_is_err() {
        let cfg = DevConfig {
            otel_workload_endpoint: Some("nats://localhost:4317".to_string()),
            ..Default::default()
        };
        assert!(
            cfg.validate()
                .unwrap_err()
                .to_string()
                .contains("dev.otel_workload_endpoint")
        );
    }

    #[test]
    fn dev_otel_protocols_deserialize_from_yaml() {
        let yaml = r#"
dev:
  wasi_otel: true
  otel_host_protocol: grpc
  otel_workload_protocol: http/protobuf
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let dev = config.dev();
        assert_eq!(dev.otel_host_protocol, Some(OtelProtocol::Grpc));
        assert_eq!(dev.otel_workload_protocol, Some(OtelProtocol::HttpProtobuf));
        assert!(dev.validate().is_ok());
    }

    #[test]
    fn dev_otel_protocol_accepts_kebab_case_alias() {
        let yaml = r#"
dev:
  otel_workload_protocol: http-protobuf
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(
            config.dev().otel_workload_protocol,
            Some(OtelProtocol::HttpProtobuf)
        );
    }

    #[test]
    fn dev_otel_protocols_absent_deserialize_to_none() {
        // Unset must stay `None`, not a default variant — that's what lets
        // `wasi_otel::OtelTarget::protocol` inherit rather than pin gRPC.
        let yaml = r#"
dev:
  wasi_otel: true
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let dev = config.dev();
        assert_eq!(dev.otel_host_protocol, None);
        assert_eq!(dev.otel_workload_protocol, None);
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
  allowedIpNameLookups:
    - "*.example.com"
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
        assert_eq!(
            workload.allowed_ip_name_lookups,
            vec!["*.example.com".parse().unwrap()]
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
    fn dev_data_nats_wrong_scheme_is_err() {
        let cfg = DevConfig {
            data_nats_url: Some("http://localhost:4222".to_string()),
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("data_nats_url"));
    }

    #[test]
    fn dev_data_nats_valid_scheme_is_ok() {
        let cfg = DevConfig {
            data_nats_url: Some("nats://127.0.0.1:4222".to_string()),
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
            components: vec![DevComponent::new("  ", "comp.wasm")],
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("dev.components"));
    }

    #[test]
    fn dev_component_empty_file_is_err() {
        let cfg = DevConfig {
            components: vec![DevComponent::new("sidecar", "")],
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("`file` source is empty"), "{err}");
    }

    #[test]
    fn dev_component_valid_is_ok() {
        let cfg = DevConfig {
            components: vec![
                DevComponent::new("sidecar", "sidecar.wasm"),
                DevComponent::from_source(
                    "pulled",
                    ComponentSourceConfig::image("ghcr.io/acme/sidecar:1"),
                ),
            ],
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn dev_component_ambiguous_source_is_err() {
        // Both sources, then neither, then a pull policy on a file.
        for source in [
            ComponentSourceConfig {
                file: Some("sidecar.wasm".into()),
                image: Some("ghcr.io/acme/sidecar:1".into()),
                pull_policy: None,
            },
            ComponentSourceConfig::default(),
            ComponentSourceConfig {
                file: Some("sidecar.wasm".into()),
                image: None,
                pull_policy: Some("always".into()),
            },
            ComponentSourceConfig {
                file: None,
                image: Some("ghcr.io/acme/sidecar:1".into()),
                pull_policy: Some("sometimes".into()),
            },
        ] {
            let cfg = DevConfig {
                components: vec![DevComponent::from_source("sidecar", source)],
                ..Default::default()
            };
            let err = cfg.validate().unwrap_err().to_string();
            assert!(err.contains("dev.components['sidecar']"), "{err}");
        }
    }

    #[test]
    fn dev_component_image_source_parses_from_yaml() {
        // `image` / `pullPolicy` are flattened alongside `file`, so a sidecar
        // names its wasm exactly the way a host plugin does.
        let yaml = r#"
dev:
  components:
    - name: sidecar
      image: ghcr.io/acme/sidecar:1.0.0
      pullPolicy: always
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let dev = config.dev();
        let source = dev.components[0].source.to_source("sidecar").unwrap();
        assert_eq!(
            source,
            ComponentSource::Oci {
                image: "ghcr.io/acme/sidecar:1.0.0".into(),
                pull_policy: OciPullPolicy::Always,
            }
        );
    }

    #[test]
    fn dev_service_takes_a_file_or_an_image_but_not_both() {
        let none = DevConfig::default();
        assert!(none.service_source().unwrap().is_none());

        let file = DevConfig {
            service_file: Some("service.wasm".into()),
            ..Default::default()
        };
        assert_eq!(
            file.service_source().unwrap(),
            Some(ComponentSource::File("service.wasm".into()))
        );

        let image = DevConfig {
            service_image: Some("ghcr.io/acme/svc:1".into()),
            service_pull_policy: Some("always".into()),
            ..Default::default()
        };
        assert_eq!(
            image.service_source().unwrap(),
            Some(ComponentSource::Oci {
                image: "ghcr.io/acme/svc:1".into(),
                pull_policy: OciPullPolicy::Always,
            })
        );

        let both = DevConfig {
            service_file: Some("service.wasm".into()),
            service_image: Some("ghcr.io/acme/svc:1".into()),
            ..Default::default()
        };
        assert!(both.service_source().is_err());
        assert!(both.validate().is_err());
    }

    #[test]
    fn dev_component_overrides_parse_from_yaml() {
        // Per-component overrides use the same camelCase shape as the
        // `workload:` block (and the k8s `localResources` they mirror).
        let yaml = r#"
dev:
  components:
    - name: hello
      file: hello.wasm
      environment:
        config:
          MY_ENV_VAR: hello
        configFrom:
          - shared
      config:
        flag: "on"
      allowedHosts:
        - https://api.example.com
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let dev = config.dev();
        let component = &dev.components[0];
        let env = component.environment.as_ref().unwrap();
        assert_eq!(env.config.get("MY_ENV_VAR").unwrap(), "hello");
        assert_eq!(env.config_from, vec!["shared".to_string()]);
        assert_eq!(component.config.get("flag").unwrap(), "on");
        assert_eq!(
            component.allowed_hosts.as_deref().unwrap(),
            &["https://api.example.com".parse().unwrap()]
        );

        // Omitting all three leaves the overrides empty (inherit workload).
        let yaml = "dev:\n  components:\n    - name: hello\n      file: hello.wasm\n";
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let dev = config.dev();
        let component = &dev.components[0];
        assert!(component.environment.is_none());
        assert!(component.config.is_empty());
        assert!(component.allowed_hosts.is_none());
    }

    #[test]
    fn dev_host_plugins_parse_from_yaml_and_convert_to_spec() {
        // The `dev.host_plugins` key follows DevConfig's snake_case; each entry's
        // fields follow the camelCase used by other nested dev structs.
        let yaml = r#"
dev:
  host_plugins:
    - id: acme-kv
      file: ./build/kv_plugin.wasm
      maxRestarts: 3
    - id: acme-widgets
      image: ghcr.io/acme/widgets:1.2.0
      pullPolicy: ifNotPresent
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let dev = config.dev();
        assert_eq!(dev.host_plugins.len(), 2);

        let kv = dev.host_plugins[0].to_spec_unresolved().unwrap();
        assert_eq!(kv.id, "acme-kv");
        assert_eq!(
            kv.source,
            ComponentSource::File("./build/kv_plugin.wasm".into())
        );
        assert_eq!(kv.max_restarts, Some(3));

        let widgets = dev.host_plugins[1].to_spec_unresolved().unwrap();
        assert_eq!(
            widgets.source,
            ComponentSource::image("ghcr.io/acme/widgets:1.2.0")
        );
    }

    #[test]
    fn host_plugins_parse_from_yaml_and_resolve_config_from_secret_from() {
        // `host.hostPlugins` mirrors `dev.host_plugins`'s shape but adds
        // `config`/`configFrom`/`secretFrom` (this plugin's own bind-time
        // config, resolved the same way `workload.environment` is) and
        // `allowedHosts`/`allowedIpNameLookups`.
        let yaml = r#"
configs:
  etcd-connection-settings:
    inline:
      etcd-prefix: /wasmcloud/secrets
secrets:
  etcd-client-cert:
    inline:
      api-key: s3cr3t-value
host:
  hostPlugins:
    - id: etcd-secrets
      image: ghcr.io/example/etcd-secrets:1.0.0
      allowedHosts:
        - https://etcd.internal:2379
      allowedIpNameLookups:
        - etcd.internal
      config:
        literal-key: literal-value
      configFrom:
        - etcd-connection-settings
      secretFrom:
        - etcd-client-cert
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let host = config.host();
        assert_eq!(host.host_plugins.len(), 1);
        let hp = &host.host_plugins[0];
        assert_eq!(hp.id, "etcd-secrets");
        assert_eq!(hp.allowed_hosts.len(), 1);
        assert_eq!(hp.allowed_ip_name_lookups.len(), 1);

        let spec = hp
            .to_spec(&config, Path::new("."), None)
            .expect("host_plugins entry should resolve");
        assert_eq!(spec.id, "etcd-secrets");
        assert_eq!(
            spec.source,
            ComponentSource::image("ghcr.io/example/etcd-secrets:1.0.0")
        );
        assert_eq!(spec.allowed_hosts.len(), 1);
        assert_eq!(spec.allowed_ip_name_lookups.len(), 1);
        // inline < configFrom < secretFrom precedence, all three present.
        assert_eq!(spec.config.get("literal-key").unwrap(), "literal-value");
        assert_eq!(
            spec.config.get("etcd-prefix").unwrap(),
            "/wasmcloud/secrets"
        );
        assert_eq!(spec.config.get("api-key").unwrap(), "s3cr3t-value");
    }

    #[test]
    fn host_plugins_unresolved_config_from_reference_is_an_error() {
        let yaml = r#"
host:
  hostPlugins:
    - id: etcd-secrets
      image: ghcr.io/example/etcd-secrets:1.0.0
      configFrom:
        - missing
"#;
        let config: Config = serde_yaml_ng::from_str(yaml).unwrap();
        let hp = &config.host().host_plugins[0];
        assert!(hp.to_spec(&config, Path::new("."), None).is_err());
    }

    #[test]
    fn host_plugin_config_validation_rejects_ambiguous_specs() {
        // Both sources set.
        let both = HostPluginConfig {
            id: "x".into(),
            source: ComponentSourceConfig {
                file: Some("a.wasm".into()),
                image: Some("ghcr.io/x:1".into()),
                pull_policy: None,
            },
            ..Default::default()
        };
        assert!(both.to_spec_unresolved().is_err());

        // No source.
        let neither = HostPluginConfig {
            id: "x".into(),
            ..Default::default()
        };
        assert!(neither.to_spec_unresolved().is_err());

        // pullPolicy with a file source.
        let pull_on_file = HostPluginConfig {
            id: "x".into(),
            source: ComponentSourceConfig {
                file: Some("a.wasm".into()),
                image: None,
                pull_policy: Some("always".into()),
            },
            ..Default::default()
        };
        assert!(pull_on_file.to_spec_unresolved().is_err());

        // Empty id.
        let empty_id = HostPluginConfig {
            source: ComponentSourceConfig::file("a.wasm"),
            ..Default::default()
        };
        assert!(empty_id.to_spec_unresolved().is_err());
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
