use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Context as _;
use clap::Args;
use tracing::info;
use wash_runtime::{
    engine::{Engine, WasmProposal},
    observability::Meters,
    plugin::{self},
};

use crate::cli::{CliCommand, CliContext, CommandOutput};

#[derive(Debug, Clone, Args)]
pub struct HostCommand {
    /// The host group label to assign to the host
    #[arg(long = "host-group", default_value = "default")]
    pub host_group: String,

    /// NATS URL for Control Plane communications
    #[arg(long = "scheduler-nats-url", default_value = "nats://localhost:4222")]
    pub scheduler_nats_url: String,

    /// Path to TLS CA certificate file for NATS Scheduler connection
    #[arg(long = "scheduler-nats-tls-ca")]
    pub scheduler_nats_tls_ca: Option<PathBuf>,

    /// Enable TLS handshake first mode for NATS Scheduler connection
    #[arg(long = "scheduler-nats-tls-first", default_value_t = false)]
    pub scheduler_nats_tls_first: bool,

    /// Path to NATS TLS certificate file for NATS Scheduler connection
    #[arg(long = "scheduler-nats-tls-cert")]
    pub scheduler_nats_tls_cert: Option<PathBuf>,

    /// Path to NATS TLS private key file for NATS Scheduler connection
    #[arg(long = "scheduler-nats-tls-key")]
    pub scheduler_nats_tls_key: Option<PathBuf>,

    /// NATS URL for Data Plane communications
    #[arg(long = "data-nats-url", default_value = "nats://localhost:4222")]
    pub data_nats_url: String,

    /// The path to TLS CA certificate file for NATS Data connection
    #[arg(long = "data-nats-tls-ca")]
    pub data_nats_tls_ca: Option<PathBuf>,

    /// Enable TLS handshake first mode for NATS Data connection
    #[arg(long = "data-nats-tls-first", default_value_t = false)]
    pub data_nats_tls_first: bool,

    /// Path to NATS TLS certificate file for NATS Data connection
    #[arg(long = "data-nats-tls-cert")]
    pub data_nats_tls_cert: Option<PathBuf>,

    /// Path to NATS TLS private key file for NATS Data connection
    #[arg(long = "data-nats-tls-key")]
    pub data_nats_tls_key: Option<PathBuf>,

    /// The host name to assign to the host
    #[arg(long = "host-name")]
    pub host_name: Option<String>,

    /// Environment the host advertises in its heartbeat. For Kubernetes
    /// host pods this is typically the pod's namespace (passed by the
    /// runtime-operator chart via the downward API). The runtime-operator
    /// records this verbatim on the resulting Host CRD's
    /// `spec.environment` field; scheduling uses it to enforce per-tenant
    /// isolation.
    #[arg(long = "environment", env = "WASMCLOUD_HOST_ENVIRONMENT")]
    pub environment: Option<String>,

    /// The address on which the HTTP server will listen
    #[arg(long = "http-addr")]
    pub http_addr: Option<SocketAddr>,

    /// Path to TLS certificate file for the HTTP server
    #[arg(long = "tls-cert-path", requires = "tls_key_path")]
    pub tls_cert_path: Option<PathBuf>,

    /// Path to TLS private key file for the HTTP server
    #[arg(long = "tls-key-path", requires = "tls_cert_path")]
    pub tls_key_path: Option<PathBuf>,

    /// Path to CA certificate file for mutual TLS on the HTTP server
    #[arg(long = "tls-ca-path")]
    pub tls_ca_path: Option<PathBuf>,

    /// Enable WASI WebGPU support
    #[cfg(all(
        not(target_os = "windows"),
        not(target_arch = "s390x"),
        feature = "wasi-webgpu"
    ))]
    #[arg(long = "wasi-webgpu", default_value_t = false)]
    pub wasi_webgpu: bool,

    /// PostgreSQL connection URL for the wasmcloud:postgres plugin
    /// (e.g. postgres://user:pass@bouncer:6432?sslmode=require&pool_size=10)
    #[arg(long = "postgres-url", env = "WASH_POSTGRES_URL")]
    pub postgres_url: Option<String>,

    /// Allow insecure OCI Registries
    #[arg(long = "allow-insecure-registries", default_value_t = false)]
    pub allow_insecure_registries: bool,

    /// Timeout for pulling artifacts from OCI registries
    #[arg(long = "registry-pull-timeout", value_parser = humantime::parse_duration, default_value = "30s")]
    pub registry_pull_timeout: Duration,

    /// The directory to use for caching OCI artifacts
    #[arg(long = "oci-cache-dir")]
    pub oci_cache_dir: Option<PathBuf>,

    /// Enable WASI OpenTelemetry plugin
    #[arg(long = "wasi-otel", default_value_t = false)]
    pub wasi_otel: bool,

    /// Enable additional wasm proposals on the engine. Accepts a comma-separated
    /// list and/or repeated flags, e.g. `--wasm-proposal gc,threads`. Accepted
    /// names: component-model-async, gc, exception-handling, wide-arithmetic,
    /// threads, tail-call.
    #[arg(
        long = "wasm-proposal",
        env = "WASH_WASM_PROPOSALS",
        value_delimiter = ','
    )]
    pub wasm_proposals: Vec<WasmProposal>,

    /// Load a host component plugin providing a host capability from its own supervised store.
    ///
    /// A WebAssembly component served to every workload that imports its
    /// interface. Repeatable; separate multiple with `;` or repeat the flag.
    /// Requires a wash build with the `host-component-plugins` feature. Each
    /// value is comma-separated `key=value` fields — required `id` and exactly
    /// one of `image`/`file`:
    ///   id=<name>,image=<oci-ref>[,pull=always|ifNotPresent|never][,max-restarts=N][,digest=sha256:..]
    ///   id=<name>,file=<path>[,max-restarts=N]
    #[arg(
        long = "host-plugin",
        env = "WASH_HOST_PLUGINS",
        value_delimiter = ';',
        value_parser = parse_host_plugin_spec
    )]
    pub host_plugins: Vec<wash_runtime::plugin::ComponentPluginSpec>,

    /// Username for authenticating to the registry when pulling host component
    /// plugins. Pair with `--host-plugin-registry-password`. Read from the
    /// environment so the credential never appears in a `--host-plugin` arg or
    /// the pod spec — in Kubernetes, source it from a Secret via `secretKeyRef`
    /// on the host container. When unset, plugin pulls fall back to the ambient
    /// docker credential helper (e.g. a mounted imagePullSecret) and then
    /// anonymous access. Applies to host-component-plugin pulls only; workload
    /// components authenticate with their own per-workload image pull secret.
    #[cfg(feature = "host-component-plugins")]
    #[arg(
        long = "host-plugin-registry-user",
        env = "WASH_HOST_PLUGIN_REGISTRY_USER",
        hide_env_values = true,
        requires = "host_plugin_registry_password"
    )]
    pub host_plugin_registry_user: Option<String>,

    /// Password paired with `--host-plugin-registry-user` /
    /// `WASH_HOST_PLUGIN_REGISTRY_USER`. Both are required together.
    #[cfg(feature = "host-component-plugins")]
    #[arg(
        long = "host-plugin-registry-password",
        env = "WASH_HOST_PLUGIN_REGISTRY_PASSWORD",
        hide_env_values = true,
        requires = "host_plugin_registry_user"
    )]
    pub host_plugin_registry_password: Option<String>,
}

/// clap value parser for `--host-plugin`: parse one spec, flattening the
/// `anyhow` error chain into the `String` clap wants.
fn parse_host_plugin_spec(s: &str) -> Result<wash_runtime::plugin::ComponentPluginSpec, String> {
    s.parse().map_err(|e: anyhow::Error| format!("{e:#}"))
}

/// Resolve explicit registry credentials for host-component-plugin pulls from
/// the CLI/env `(user, password)` pair. The two are required together (clap
/// `requires`); a lone half — reachable only defensively — is treated as no
/// credentials, leaving the pull to fall back to the docker credential helper
/// and then anonymous.
#[cfg(feature = "host-component-plugins")]
fn host_plugin_registry_credentials(
    user: Option<&str>,
    password: Option<&str>,
) -> Option<(String, String)> {
    match (user, password) {
        (Some(user), Some(password)) => Some((user.to_string(), password.to_string())),
        (None, None) => None,
        (Some(_), None) | (None, Some(_)) => None,
    }
}

impl CliCommand for HostCommand {
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        // Installed before connect_nats so TLS-enabled NATS clusters have a
        // crypto provider available. Idempotent; also called by HttpServer::new.
        wash_runtime::init_crypto();

        let scheduler_nats_client = wash_runtime::washlet::connect_nats(
            self.scheduler_nats_url.clone(),
            wash_runtime::washlet::NatsConnectionOptions {
                request_timeout: None,
                tls_ca: self.scheduler_nats_tls_ca.clone(),
                tls_first: self.scheduler_nats_tls_first,
                tls_cert: self.scheduler_nats_tls_cert.clone(),
                tls_key: self.scheduler_nats_tls_key.clone(),
            },
        )
        .await
        .context("failed to connect to NATS Scheduler URL")?;

        let data_nats_client = wash_runtime::washlet::connect_nats(
            self.data_nats_url.clone(),
            wash_runtime::washlet::NatsConnectionOptions {
                request_timeout: None,
                tls_ca: self.data_nats_tls_ca.clone(),
                tls_first: self.data_nats_tls_first,
                tls_cert: self.data_nats_tls_cert.clone(),
                tls_key: self.data_nats_tls_key.clone(),
            },
        )
        .await
        .context("failed to connect to NATS")?;
        let data_nats_client = Arc::new(data_nats_client);

        let host_config = wash_runtime::host::HostConfig {
            allow_oci_insecure: self.allow_insecure_registries,
            oci_pull_timeout: Some(self.registry_pull_timeout),
            oci_cache_dir: self.oci_cache_dir.clone(),
        };

        let mut engine_builder = Engine::builder()
            .with_pooling_allocator(true)
            .with_fuel_consumption(ctx.enable_meters());
        for proposal in &self.wasm_proposals {
            engine_builder = engine_builder.with_wasm_proposal(*proposal);
        }
        let engine = engine_builder.build()?;

        let mut cluster_host_builder = wash_runtime::washlet::ClusterHostBuilder::default()
            .with_engine(engine.clone())
            .with_host_config(host_config)
            .with_nats_client(Arc::new(scheduler_nats_client))
            .with_host_group(self.host_group.clone())
            .with_plugin(Arc::new(
                plugin::wasi_config::DynamicConfig::builder()
                    .copy_environment(true)
                    .build(),
            ))?
            .with_plugin(Arc::new(plugin::wasi_logging::TracingLogger::default()))?
            .with_plugin(Arc::new(plugin::wasi_blobstore::NatsBlobstore::new(
                &data_nats_client,
            )))?
            .with_plugin(Arc::new(plugin::wasmcloud_messaging::NatsMessaging::new(
                data_nats_client.clone(),
            )))?
            .with_plugin(Arc::new(plugin::wasi_keyvalue::NatsKeyValue::new(
                &data_nats_client,
            )))?
            .with_meters(Meters::new(ctx.enable_meters()));

        #[cfg(feature = "wasm_component_model_implements")]
        {
            cluster_host_builder = cluster_host_builder.with_plugin(Arc::new(
                plugin::wasi_keyvalue::MultiplexedKeyValue::new()
                    .with_provider(Arc::new(plugin::wasi_keyvalue::InMemoryProvider))
                    .with_provider(Arc::new(plugin::wasi_keyvalue::RedisProvider))
                    .with_provider(Arc::new(plugin::wasi_keyvalue::NatsProvider))
                    .with_provider(Arc::new(plugin::wasi_keyvalue::FilesystemProvider)),
            ))?;
            cluster_host_builder = cluster_host_builder.with_plugin(Arc::new(
                plugin::wasmcloud_messaging::MultiplexedMessaging::new()
                    .with_provider(Arc::new(plugin::wasmcloud_messaging::InMemoryMsgProvider))
                    .with_provider(Arc::new(plugin::wasmcloud_messaging::NatsMsgProvider)),
            ))?;
            cluster_host_builder = cluster_host_builder.with_plugin(Arc::new(
                plugin::wasi_blobstore::MultiplexedBlobstore::new()
                    .with_provider(Arc::new(plugin::wasi_blobstore::InMemoryProvider))
                    .with_provider(Arc::new(plugin::wasi_blobstore::FilesystemProvider))
                    .with_provider(Arc::new(plugin::wasi_blobstore::NatsBlobProvider)),
            ))?;
            cluster_host_builder = cluster_host_builder.with_plugin(Arc::new(
                plugin::wasi_blobstore::MultiplexedAsyncBlobstore::new()
                    .with_provider(Arc::new(plugin::wasi_blobstore::InMemoryProvider))
                    .with_provider(Arc::new(plugin::wasi_blobstore::FilesystemProvider))
                    .with_provider(Arc::new(plugin::wasi_blobstore::NatsBlobProvider)),
            ))?;
            cluster_host_builder = cluster_host_builder.with_plugin(Arc::new(
                plugin::wasi_keyvalue::MultiplexedAsyncKeyValue::new()
                    .with_provider(Arc::new(plugin::wasi_keyvalue::InMemoryProvider))
                    .with_provider(Arc::new(plugin::wasi_keyvalue::RedisProvider))
                    .with_provider(Arc::new(plugin::wasi_keyvalue::NatsProvider))
                    .with_provider(Arc::new(plugin::wasi_keyvalue::FilesystemProvider)),
            ))?;
        }

        if let Some(postgres_url) = &self.postgres_url {
            cluster_host_builder = cluster_host_builder.with_plugin(Arc::new(
                plugin::wasmcloud_postgres::WasmcloudPostgres::new(postgres_url)
                    .context("failed to configure postgres plugin")?,
            ))?;
        } else {
            // register postgres for `(implements ..)` named imports (each
            // carrying its own URL) are served.
            #[cfg(feature = "wasm_component_model_implements")]
            {
                cluster_host_builder = cluster_host_builder.with_plugin(Arc::new(
                    plugin::wasmcloud_postgres::WasmcloudPostgres::multiplex_only(),
                ))?;
            }
        }

        if let Some(host_name) = &self.host_name {
            cluster_host_builder = cluster_host_builder.with_host_name(host_name);
        }

        if let Some(environment) = &self.environment {
            cluster_host_builder = cluster_host_builder.with_environment(environment);
        }

        if let Some(addr) = self.http_addr {
            let http_router = wash_runtime::host::http::DynamicRouter::default();
            let http_server = if let (Some(cert_path), Some(key_path)) =
                (&self.tls_cert_path, &self.tls_key_path)
            {
                let mut tls = wash_runtime::host::http::TlsConfig::new(cert_path, key_path);
                if let Some(ca) = self.tls_ca_path.as_deref() {
                    tls = tls.with_ca(ca);
                }
                wash_runtime::host::http::HttpServer::new_with_tls(http_router, addr, tls).await?
            } else {
                wash_runtime::host::http::HttpServer::new(http_router, addr).await?
            };
            cluster_host_builder = cluster_host_builder.with_http_handler(Arc::new(http_server));
        }

        // Enable otel plugin
        if self.wasi_otel {
            cluster_host_builder = cluster_host_builder
                .with_plugin(Arc::new(plugin::wasi_otel::WasiOtel::default()))?;
        }

        // Enable WASI WebGPU if requested
        #[cfg(all(
            not(target_os = "windows"),
            not(target_arch = "s390x"),
            feature = "wasi-webgpu"
        ))]
        if self.wasi_webgpu {
            tracing::info!("WASI WebGPU support enabled");
            cluster_host_builder = cluster_host_builder
                .with_plugin(Arc::new(plugin::wasi_webgpu::WebGpu::default()))?;
        }

        // Host component plugins: fetch each declared plugin's wasm and register
        // it before the host starts. Host-operator controlled only — nothing in a
        // workload request can register a host-global capability provider.
        #[cfg(feature = "host-component-plugins")]
        {
            // Explicit registry credentials for plugin pulls, taken from the
            // environment so the secret never appears in a --host-plugin arg or
            // the pod spec. When unset, resolution falls back to the ambient
            // docker credential helper (e.g. a mounted imagePullSecret) and then
            // anonymous access.
            let plugin_oci_config = wash_runtime::oci::OciConfig {
                credentials: host_plugin_registry_credentials(
                    self.host_plugin_registry_user.as_deref(),
                    self.host_plugin_registry_password.as_deref(),
                ),
                insecure: self.allow_insecure_registries,
                cache_dir: self.oci_cache_dir.clone(),
                timeout: Some(self.registry_pull_timeout),
            };
            for spec in &self.host_plugins {
                let plugin = wash_runtime::plugin::component_host::load_component_plugin(
                    spec,
                    &engine,
                    plugin_oci_config.clone(),
                )
                .await
                .with_context(|| format!("failed to load host component plugin '{}'", spec.id))?;
                cluster_host_builder = cluster_host_builder.with_plugin(plugin)?;
                info!(id = %spec.id, "loaded host component plugin");
            }
        }
        #[cfg(not(feature = "host-component-plugins"))]
        anyhow::ensure!(
            self.host_plugins.is_empty(),
            "--host-plugin/WASH_HOST_PLUGINS requires a wash build with the \
             `host-component-plugins` feature"
        );

        let cluster_host = cluster_host_builder
            .build()
            .context("failed to build cluster host")?;
        let host_cleanup = wash_runtime::washlet::run_cluster_host(cluster_host)
            .await
            .context("failed to start cluster node")?;

        tokio::signal::ctrl_c()
            .await
            .context("failed to listen for shutdown signal")?;

        info!("Stopping host...");

        host_cleanup.await?;

        Ok(CommandOutput::ok(
            "Host exited successfully".to_string(),
            None,
        ))
    }
}

#[cfg(all(test, feature = "host-component-plugins"))]
mod tests {
    use super::host_plugin_registry_credentials;

    #[test]
    fn both_halves_yield_credentials() {
        assert_eq!(
            host_plugin_registry_credentials(Some("user"), Some("pass")),
            Some(("user".to_string(), "pass".to_string())),
        );
    }

    #[test]
    fn neither_half_yields_no_credentials() {
        assert_eq!(host_plugin_registry_credentials(None, None), None);
    }

    #[test]
    fn a_half_pair_is_ignored_not_half_applied() {
        // Only a username, or only a password, must resolve to no explicit
        // credentials — never a basic auth with an empty half.
        assert_eq!(host_plugin_registry_credentials(Some("user"), None), None);
        assert_eq!(host_plugin_registry_credentials(None, Some("pass")), None);
    }
}
