use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use anyhow::{Context as _, bail, ensure};
use bytes::Bytes;
use clap::Args;
use tokio::{select, sync::mpsc};
use tracing::{debug, info, instrument, warn};
use wash_runtime::{
    engine::{Engine, WasmProposal},
    host::{Host, HostApi},
    observability::Meters,
    plugin::{self},
    types::{
        Component, HostPathVolume, LocalResources, Service, Volume, VolumeMount, VolumeType,
        Workload, WorkloadStartRequest, WorkloadState, WorkloadStopRequest,
    },
    wit::WitInterface,
};

use crate::{
    cli::{CliCommand, CliContext, CommandOutput, component_build::build_dev_component},
    config::{Config, load_config},
    wit::WitConfig,
    workload::{ResolvedWorkload, resolve_component_workload, resolve_workload},
};

/// Start a development server for a Wasm component
#[derive(Debug, Clone, Args)]
pub struct DevCommand {}

impl CliCommand for DevCommand {
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        wash_runtime::init_crypto();

        let project_dir = ctx.project_dir();
        info!(path = ?project_dir, "starting development session for project");

        // Load configuration with wit fetch disabled for 2 reasons:
        // 1. During dev sessions, we want to avoid network calls for fetching WIT files to ensure faster startup times.
        // 2. It can cause file watchers to enter a build-loop as we touch wkg files during fetch.
        let config = load_config(
            &ctx.user_config_path(),
            Some(project_dir),
            Some(Config {
                dev: None,
                wit: Some(WitConfig {
                    skip_fetch: true,
                    ..Default::default()
                }),
                ..Default::default()
            }),
        )
        .context("failed to load config for development")?;

        let dev_config = config.dev();

        let http_addr = dev_config
            .address
            .clone()
            .unwrap_or_else(|| "0.0.0.0:8000".to_string());

        let mut engine_builder = Engine::builder()
            .with_pooling_allocator(true)
            .with_fuel_consumption(ctx.enable_meters())
            .with_allow_ip_name_lookup(dev_config.allow_ip_name_lookup);
        for name in &dev_config.wasm_proposals {
            let proposal: WasmProposal = name
                .parse()
                .with_context(|| format!("invalid dev.wasm_proposals entry {name:?}"))?;
            engine_builder = engine_builder.with_wasm_proposal(proposal);
        }
        let engine = engine_builder.build()?;

        let mut host_builder = Host::builder()
            .with_engine(engine.clone())
            .with_meters(Meters::new(ctx.enable_meters()));

        // Enable wasi config. `copy_environment = true` surfaces each
        // component's `LocalResources.environment` via `wasi:config/store`,
        // so resolved env vars are visible as both env vars and wasi:config
        // entries without further plumbing.
        host_builder = host_builder.with_plugin(Arc::new(
            plugin::wasi_config::DynamicConfig::builder()
                .copy_environment(true)
                .build(),
        ))?;

        // Shared data-plane NATS connection, mirroring `wash host`
        // `--data-nats-url`. When `dev.data_nats_url` is set it backs
        // blobstore, keyvalue, and messaging unless a per-plugin config overrides
        // it. Connected once and shared across the three plugins.
        let data_nats_client = if let Some(url) = &dev_config.data_nats_url {
            let client = async_nats::connect(url.as_str())
                .await
                .context("failed to connect to NATS for dev.data_nats_url")?;
            Some(Arc::new(client))
        } else {
            None
        };

        // Enable wasmcloud:messaging — NATS when data_nats_url is configured,
        // otherwise the in-memory backend.
        if let Some(client) = &data_nats_client {
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasmcloud_messaging::NatsMessaging::new(client.clone()),
            ))?;
            debug!("wasmcloud:messaging plugin registered with NATS backend (data_nats_url)");
        } else {
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasmcloud_messaging::InMemoryMessaging::default(),
            ))?;
            debug!("wasmcloud:messaging plugin registered with in-memory backend");
        }

        // Per-plugin settings override the in-memory default. The order of precedence is:
        // use a filesystem backend if there is a path override,
        // otherwise it uses NATS if `data_nats_url` is set, otherwise it falls back to
        // the in-memory plugin.
        if let Some(blobstore_path) = &dev_config.wasi_blobstore_path {
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasi_blobstore::FilesystemBlobstore::new(blobstore_path.clone()),
            ))?;
            debug!(
                path = %blobstore_path.display(),
                "WASI Blobstore plugin registered with filesystem backend"
            );
        } else if let Some(client) = &data_nats_client {
            host_builder = host_builder
                .with_plugin(Arc::new(plugin::wasi_blobstore::NatsBlobstore::new(client)))?;
            debug!("WASI Blobstore plugin registered with NATS backend (data_nats_url)");
        } else {
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasi_blobstore::InMemoryBlobstore::default(),
            ))?;
            debug!("WASI Blobstore plugin registered with in-memory backend");
        }

        // Host component plugins: WebAssembly components that provide host
        // capabilities, each in its own supervised store. Fetched (local file or
        // OCI) and registered before the host starts.
        #[cfg(feature = "host-component-plugins")]
        for hp in &dev_config.host_plugins {
            let spec = hp.to_spec()?;
            let plugin = wash_runtime::plugin::component_host::load_component_plugin(
                &spec,
                &engine,
                wash_runtime::oci::OciConfig::default(),
            )
            .await
            .with_context(|| format!("failed to load host component plugin '{}'", spec.id))?;
            host_builder = host_builder.with_plugin(plugin)?;
            debug!(id = %spec.id, "host component plugin registered");
        }
        #[cfg(not(feature = "host-component-plugins"))]
        ensure!(
            dev_config.host_plugins.is_empty(),
            "dev.host_plugins requires a wash build with the `host-component-plugins` feature"
        );

        let http_handler = wash_runtime::host::http::DevRouter::default();
        // TODO(#19): Only spawn the server if the component exports wasi:http
        // Configure HTTP server with optional TLS, enable HTTP Server
        let protocol = if let (Some(cert_path), Some(key_path)) =
            (&dev_config.tls_cert_path, &dev_config.tls_key_path)
        {
            ensure!(
                cert_path.exists(),
                "TLS certificate file does not exist: {}",
                cert_path.display()
            );
            ensure!(
                key_path.exists(),
                "TLS private key file does not exist: {}",
                key_path.display()
            );

            if let Some(ca_path) = &dev_config.tls_ca_path {
                ensure!(
                    ca_path.exists(),
                    "CA certificate file does not exist: {}",
                    ca_path.display()
                );
            }

            let mut tls = wash_runtime::host::http::TlsConfig::new(cert_path, key_path);
            if let Some(ca) = dev_config.tls_ca_path.as_deref() {
                tls = tls.with_ca(ca);
            }
            let http_server = wash_runtime::host::http::HttpServer::new_with_tls(
                http_handler,
                http_addr.parse()?,
                tls,
            )
            .await?;

            host_builder = host_builder.with_http_handler(Arc::new(http_server));

            debug!("TLS configured - server will use HTTPS");
            "https"
        } else {
            debug!("No TLS configuration provided - server will use HTTP");
            let http_server =
                wash_runtime::host::http::HttpServer::new(http_handler, http_addr.parse()?).await?;
            host_builder = host_builder.with_http_handler(Arc::new(http_server));
            "http"
        };

        // Add logging plugin
        host_builder =
            host_builder.with_plugin(Arc::new(plugin::wasi_logging::TracingLogger::default()))?;
        debug!("Logging plugin registered");

        // Add keyvalue plugin — Redis > NATS override > filesystem >
        // NATS (data_nats_url) > in-memory. Per-plugin settings win over the
        // shared data_nats_url default.
        if let Some(redis_url) = &dev_config.wasi_keyvalue_redis_url {
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasi_keyvalue::RedisKeyValue::from_url(redis_url)
                    .context("failed to configure Redis keyvalue plugin")?,
            ))?;
            debug!(url = %redis_url, "WASI KeyValue plugin registered with Redis backend");
        } else if let Some(nats_url) = &dev_config.wasi_keyvalue_nats_url {
            let nats_client = async_nats::connect(nats_url.as_str())
                .await
                .context("failed to connect to NATS for keyvalue plugin")?;
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasi_keyvalue::NatsKeyValue::new(&nats_client),
            ))?;
            debug!(url = %nats_url, "WASI KeyValue plugin registered with NATS backend");
        } else if let Some(keyvalue_path) = &dev_config.wasi_keyvalue_path {
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasi_keyvalue::FilesystemKeyValue::new(keyvalue_path.clone()),
            ))?;
            debug!(
                path = %keyvalue_path.display(),
                "WASI KeyValue plugin registered with filesystem backend"
            );
        } else if let Some(client) = &data_nats_client {
            host_builder = host_builder
                .with_plugin(Arc::new(plugin::wasi_keyvalue::NatsKeyValue::new(client)))?;
            debug!("WASI KeyValue plugin registered with NATS backend (data_nats_url)");
        } else {
            host_builder = host_builder
                .with_plugin(Arc::new(plugin::wasi_keyvalue::InMemoryKeyValue::default()))?;
            debug!("WASI KeyValue plugin registered with in-memory backend");
        }

        #[cfg(feature = "wasm_component_model_implements")]
        {
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasi_keyvalue::MultiplexedKeyValue::new()
                    .with_provider(Arc::new(plugin::wasi_keyvalue::InMemoryProvider))
                    .with_provider(Arc::new(plugin::wasi_keyvalue::RedisProvider))
                    .with_provider(Arc::new(plugin::wasi_keyvalue::NatsProvider))
                    .with_provider(Arc::new(plugin::wasi_keyvalue::FilesystemProvider)),
            ))?;
            debug!("WASI KeyValue multiplexed plugin registered (implements)");
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasmcloud_messaging::MultiplexedMessaging::new()
                    .with_provider(Arc::new(plugin::wasmcloud_messaging::InMemoryMsgProvider))
                    .with_provider(Arc::new(plugin::wasmcloud_messaging::NatsMsgProvider)),
            ))?;
            debug!("wasmcloud:messaging multiplexed plugin registered (implements)");
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasi_blobstore::MultiplexedBlobstore::new()
                    .with_provider(Arc::new(plugin::wasi_blobstore::InMemoryProvider))
                    .with_provider(Arc::new(plugin::wasi_blobstore::FilesystemProvider))
                    .with_provider(Arc::new(plugin::wasi_blobstore::NatsBlobProvider)),
            ))?;
            debug!("wasi:blobstore multiplexed plugin registered (implements)");
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasi_blobstore::MultiplexedAsyncBlobstore::new()
                    .with_provider(Arc::new(plugin::wasi_blobstore::InMemoryProvider))
                    .with_provider(Arc::new(plugin::wasi_blobstore::FilesystemProvider))
                    .with_provider(Arc::new(plugin::wasi_blobstore::NatsBlobProvider)),
            ))?;
            debug!("wasmcloud:blobstore async multiplexed plugin registered (implements)");
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasi_keyvalue::MultiplexedAsyncKeyValue::new()
                    .with_provider(Arc::new(plugin::wasi_keyvalue::InMemoryProvider))
                    .with_provider(Arc::new(plugin::wasi_keyvalue::RedisProvider))
                    .with_provider(Arc::new(plugin::wasi_keyvalue::NatsProvider))
                    .with_provider(Arc::new(plugin::wasi_keyvalue::FilesystemProvider)),
            ))?;
            debug!("wasmcloud:keyvalue async multiplexed plugin registered (implements)");
        }

        // Add postgres plugin if configured
        if let Some(postgres_url) = &dev_config.postgres_url {
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasmcloud_postgres::WasmcloudPostgres::new(postgres_url)
                    .context("failed to configure postgres plugin")?,
            ))?;
            debug!("wasmcloud:postgres plugin registered");
        } else {
            // No shared bouncer URL: still register postgres so workloads that
            // route purely through `(implements ..)` named imports (each
            // carrying its own URL) are served.
            #[cfg(feature = "wasm_component_model_implements")]
            {
                host_builder = host_builder.with_plugin(Arc::new(
                    plugin::wasmcloud_postgres::WasmcloudPostgres::multiplex_only(),
                ))?;
                debug!("wasmcloud:postgres multiplexed plugin registered (implements)");
            }
        }

        // Add otel plugin
        if dev_config.wasi_otel {
            host_builder =
                host_builder.with_plugin(Arc::new(plugin::wasi_otel::WasiOtel::default()))?;
            debug!("WASI OpenTelemetry plugin registered");
        }

        // Enable WASI WebGPU if requested
        #[cfg(all(
            not(target_os = "windows"),
            not(target_arch = "s390x"),
            feature = "wasi-webgpu"
        ))]
        if dev_config.wasi_webgpu {
            host_builder =
                host_builder.with_plugin(Arc::new(plugin::wasi_webgpu::WebGpu::default()))?;
            debug!("WASI WebGPU plugin registered");
        }

        // Build and start the host
        let host = host_builder.build()?.start().await?;
        host.log_interfaces();

        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);

        // Spawn a task to handle Ctrl + C signal
        tokio::spawn(async move {
            tokio::signal::ctrl_c()
                .await
                .context("failed to wait for ctrl_c signal")?;
            stop_tx
                .send(())
                .await
                .context("failed to send stop signal after receiving Ctrl + c")?;
            Result::<_, anyhow::Error>::Ok(())
        });

        info!("development session started, building and deploying component...");

        let build_result = build_dev_component(ctx, &config)
            .await
            .context("failed to build component")?;

        debug!(
            component_path = ?build_result.component_path.display(),
            "using component path for dev session"
        );
        // Deploy to local host
        let wasm_bytes = tokio::fs::read(&build_result.component_path)
            .await
            .context("failed to read component file")?;
        // Resolve workload-level env / config / allowed_hosts now (before
        // running the component) so that bad config or missing secrets fail
        // before we deploy. Pass project_dir as the repo root for the
        // gitignored-secret warning.
        let resolved_workload = resolve_workload(&config, project_dir, Some(project_dir))
            .context("failed to resolve workload-level configuration")?;
        let workload = create_workload(
            &host,
            &config,
            project_dir,
            wasm_bytes.into(),
            &resolved_workload,
        )
        .await?;
        // Running workload ID for reloads
        let workload_id = reload_component(host.clone(), &workload, None).await?;

        // Display 127.0.0.1 instead of 0.0.0.0 for user-friendly clickable URL
        let display_addr = http_addr.replace("0.0.0.0", "127.0.0.1");
        info!(address = %format!("{}://{}", protocol, display_addr), "listening for HTTP requests");

        select! {
            // Process a stop
            _ = stop_rx.recv() => {
                info!("Stopping development session ...");
            },
        }

        // Stop the workload and clean up resources
        if let Err(e) = host
            .workload_stop(WorkloadStopRequest {
                workload_id: workload_id.clone(),
            })
            .await
        {
            warn!(
                workload_id = workload_id,
                error = ?e,
                "failed to stop workload during shutdown, continuing cleanup"
            );
        } else {
            debug!(workload_id = workload_id, "workload stopped successfully");
        }

        Ok(CommandOutput::ok(
            "Development command executed successfully".to_string(),
            None,
        ))
    }
}

/// Loaded inputs for [`build_workload`]: a component's raw bytes, its
/// extracted WIT interface set, and its resolved workload values (workload
/// base merged with the component's overrides), so the pure assembly step
/// needs no further I/O or host calls.
struct LoadedComponent {
    name: String,
    bytes: Bytes,
    interfaces: HashSet<WitInterface>,
    workload: ResolvedWorkload,
}

/// Thin wrapper around [`build_workload`]: extracts dev-component
/// interfaces, loads sidecar bytes + interfaces, resolves each sidecar's
/// overrides over the workload-level base, and (when configured) loads the
/// service-file bytes. All workload-construction logic lives in
/// `build_workload`.
async fn create_workload(
    host: &Host,
    config: &Config,
    project_dir: &Path,
    bytes: Bytes,
    resolved_workload: &ResolvedWorkload,
) -> anyhow::Result<Workload> {
    let dev_config = config.dev();

    let dev_interfaces = host
        .intersect_interfaces(&bytes)
        .context("failed to extract component interfaces")?;

    let mut sidecars = Vec::with_capacity(dev_config.components.len());
    for dev_component in &dev_config.components {
        let comp_bytes = tokio::fs::read(&dev_component.file)
            .await
            .with_context(|| {
                format!(
                    "failed to read component file at {}",
                    dev_component.file.display()
                )
            })?;
        let interfaces = host
            .intersect_interfaces(&comp_bytes)
            .context("failed to extract component interfaces")?;
        // Errors already name the component.
        let workload = resolve_component_workload(
            resolved_workload,
            dev_component,
            config,
            project_dir,
            Some(project_dir),
        )?;
        sidecars.push(LoadedComponent {
            name: dev_component.name.clone(),
            bytes: Bytes::from(comp_bytes),
            interfaces,
            workload,
        });
    }

    // The service file is only deployed as a service when the dev component
    // isn't itself the service (`dev.service = false`); see `build_workload`.
    // When `dev.service` is true the file is ignored, so there's no point
    // reading it or folding its imports into the workload host interfaces.
    let (service_file_bytes, service_interfaces) = match &dev_config.service_file {
        Some(service_path) if !dev_config.service => {
            let raw = tokio::fs::read(service_path).await.with_context(|| {
                format!("failed to read service file at {}", service_path.display())
            })?;
            let interfaces = host
                .intersect_interfaces(&raw)
                .context("failed to extract service file interfaces")?;
            (Some(Bytes::from(raw)), Some(interfaces))
        }
        _ => (None, None),
    };

    Ok(build_workload(
        &dev_config,
        bytes,
        dev_interfaces,
        sidecars,
        service_file_bytes,
        service_interfaces,
        resolved_workload,
    ))
}

/// Pure assembly of a [`Workload`] from already-loaded inputs.
///
/// This is the function `wash dev` actually uses to construct the workload
/// it ships to the host. `create_workload` is just I/O around it. Keeping
/// it pure lets the unit tests verify the real production codepath:
/// per-component LocalResources placement, sidecar handling, the service
/// vs component branch, volume wiring, and the wasi:config injection (via
/// [`build_workload_host_interfaces`]).
///
/// Workload values (`environment`, `config`, `allowed_hosts`) land on every
/// component and service: the dev component and any service carry the
/// workload-level values; each `dev.components` sidecar carries its own
/// pre-merged values from [`create_workload`]. Workload-level decisions
/// (notably the `wasi:config` interface injection) consider every
/// component's imports, including sidecars.
fn build_workload(
    dev_config: &crate::config::DevConfig,
    bytes: Bytes,
    dev_interfaces: HashSet<WitInterface>,
    sidecars: Vec<LoadedComponent>,
    service_file_bytes: Option<Bytes>,
    service_interfaces: Option<HashSet<WitInterface>>,
    resolved_workload: &ResolvedWorkload,
) -> Workload {
    let mut volumes = Vec::<Volume>::new();
    let mut volume_mounts = Vec::<VolumeMount>::new();

    for cfg_volume in &dev_config.volumes {
        let name = uuid::Uuid::new_v4().to_string();
        volumes.push(Volume {
            name: name.clone(),
            volume_type: VolumeType::HostPath(HostPathVolume {
                local_path: cfg_volume.host_path.to_string_lossy().to_string(),
            }),
        });
        volume_mounts.push(VolumeMount {
            name,
            mount_path: cfg_volume.guest_path.to_string_lossy().to_string(),
            read_only: false,
        });
    }

    // dev component + sidecars + optional service file.
    let mut all_component_interfaces = Vec::with_capacity(2 + sidecars.len());
    all_component_interfaces.push(dev_interfaces);
    for s in &sidecars {
        all_component_interfaces.push(s.interfaces.clone());
    }
    if let Some(svc_interfaces) = service_interfaces {
        all_component_interfaces.push(svc_interfaces);
    }

    let host_interfaces = build_workload_host_interfaces(
        dev_config.host_interfaces.clone(),
        &all_component_interfaces,
        &resolved_workload.config,
    );

    let local_resources_for = |w: &ResolvedWorkload| LocalResources {
        volume_mounts: volume_mounts.clone(),
        environment: w.environment.clone(),
        config: w.config.clone(),
        allowed_hosts: w.allowed_hosts.clone().into(),
        ..Default::default()
    };

    let mut service: Option<Service> = None;
    let mut components = Vec::new();
    if dev_config.service {
        service = Some(Service {
            bytes,
            digest: None,
            max_restarts: 0,
            local_resources: local_resources_for(resolved_workload),
        })
    } else {
        components.push(Component {
            name: "wash-dev-component".to_string(),
            bytes,
            digest: None,
            local_resources: local_resources_for(resolved_workload),
            pool_size: -1,
            max_invocations: -1,
        });

        if let Some(service_bytes) = service_file_bytes {
            service = Some(Service {
                bytes: service_bytes,
                digest: None,
                max_restarts: 0,
                local_resources: local_resources_for(resolved_workload),
            });
        }
    }

    for sidecar in sidecars {
        components.push(Component {
            name: sidecar.name,
            bytes: sidecar.bytes,
            digest: None,
            local_resources: local_resources_for(&sidecar.workload),
            pool_size: -1,
            max_invocations: -1,
        });
    }

    debug!("workload host interfaces: {:?}", host_interfaces);

    Workload {
        namespace: "default".to_string(),
        name: "dev".to_string(),
        annotations: HashMap::default(),
        components,
        host_interfaces,
        service,
        volumes,
    }
}

/// Merge per-component WIT interface sets into a workload-level
/// `host_interfaces` list, optionally injecting `workload.config` values into
/// the `wasi:config` entry.
///
/// The wasi:config entry is workload-scoped (one per workload, not per
/// component), so injection happens at most once even when multiple
/// components import wasi:config. They all read from the same map. When NO
/// component in the workload imports wasi:config the entry is not created at
/// all, to avoid dead state and to leave room for an explicit declaration in
/// `dev.host_interfaces`. When the user has already declared a wasi:config
/// entry in `dev.host_interfaces`, their values win on key conflicts.
///
/// `base` is the user's `dev.host_interfaces` (the explicit declarations);
/// `component_interfaces` is one set per component in the workload, in any
/// order; `workload_config` is the resolved `workload.config` map.
fn build_workload_host_interfaces(
    mut base: Vec<WitInterface>,
    component_interfaces: &[HashSet<WitInterface>],
    workload_config: &HashMap<String, String>,
) -> Vec<WitInterface> {
    let mut any_imports_wasi_config = false;
    for set in component_interfaces {
        for interface in set {
            if interface.namespace == "wasi" && interface.package == "config" {
                any_imports_wasi_config = true;
            }
            if !base
                .iter()
                .any(|i| i.namespace == interface.namespace && i.package == interface.package)
            {
                base.push(interface.clone());
            }
        }
    }

    if any_imports_wasi_config && !workload_config.is_empty() {
        match base
            .iter_mut()
            .find(|i| i.namespace == "wasi" && i.package == "config")
        {
            Some(existing) => {
                for (k, v) in workload_config {
                    existing
                        .config
                        .entry(k.clone())
                        .or_insert_with(|| v.clone());
                }
            }
            None => {
                base.push(WitInterface {
                    namespace: "wasi".into(),
                    package: "config".into(),
                    interfaces: Default::default(),
                    version: None,
                    config: workload_config.clone(),
                    name: None,
                });
            }
        }
    }

    base
}

/// Reload the component in the host, stopping the previous workload if needed
#[instrument(name = "reload_component", skip_all, fields(workload_id = ?workload_id))]
async fn reload_component(
    host: Arc<Host>,
    workload: &Workload,
    workload_id: Option<String>,
) -> anyhow::Result<String> {
    if let Some(workload_id) = workload_id {
        host.workload_stop(WorkloadStopRequest { workload_id })
            .await?;
    }

    let response = host
        .workload_start(WorkloadStartRequest {
            workload_id: uuid::Uuid::new_v4().to_string(),
            workload: workload.to_owned(),
        })
        .await?;

    if response.workload_status.workload_state != WorkloadState::Running {
        bail!(
            "failed to reload component: {}",
            response.workload_status.message
        );
    }

    Ok(response.workload_status.workload_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DevComponent, DevConfig, DevVolume};
    use std::path::PathBuf;

    fn iface(namespace: &str, package: &str) -> WitInterface {
        WitInterface {
            namespace: namespace.into(),
            package: package.into(),
            interfaces: HashSet::new(),
            version: None,
            config: HashMap::new(),
            name: None,
        }
    }

    fn iface_with_config(namespace: &str, package: &str, kvs: &[(&str, &str)]) -> WitInterface {
        let mut i = iface(namespace, package);
        for (k, v) in kvs {
            i.config.insert((*k).into(), (*v).into());
        }
        i
    }

    fn find_iface<'a>(
        list: &'a [WitInterface],
        namespace: &str,
        package: &str,
    ) -> Option<&'a WitInterface> {
        list.iter()
            .find(|i| i.namespace == namespace && i.package == package)
    }

    /// Cheap stand-in for "real wasm bytes". `build_workload` never inspects
    /// the bytes; it just threads them into the Component/Service struct.
    fn fake_bytes(tag: &str) -> Bytes {
        Bytes::from(tag.as_bytes().to_vec())
    }

    fn dev_component_named(name: &str) -> DevComponent {
        DevComponent::new(name, format!("/dev/null/{name}.wasm"))
    }

    /// A sidecar input with the given pre-merged workload values, as
    /// `create_workload` would produce it.
    fn loaded_sidecar(name: &str, workload: ResolvedWorkload) -> LoadedComponent {
        LoadedComponent {
            name: name.into(),
            bytes: fake_bytes(name),
            interfaces: HashSet::new(),
            workload,
        }
    }

    /// Find a component by name in a built Workload.
    fn find_component<'a>(workload: &'a Workload, name: &str) -> Option<&'a Component> {
        workload.components.iter().find(|c| c.name == name)
    }

    #[test]
    fn build_workload_lands_workload_values_on_every_component() {
        // Workload values (environment / config / allowed_hosts) are
        // workload-wide: the dev component and every sidecar receive them.
        // A sidecar without overrides carries the workload values unchanged.
        let resolved = ResolvedWorkload {
            environment: HashMap::from([("LOG".into(), "debug".into())]),
            config: HashMap::from([("flag".into(), "on".into())]),
            allowed_hosts: vec!["https://api.example.com".parse().unwrap()],
        };
        let dev_cfg = DevConfig {
            components: vec![dev_component_named("sidecar-a")],
            ..Default::default()
        };
        let sidecars = vec![loaded_sidecar("sidecar-a", resolved.clone())];

        let workload = build_workload(
            &dev_cfg,
            fake_bytes("dev"),
            HashSet::new(),
            sidecars,
            None,
            None,
            &resolved,
        );

        let dev = find_component(&workload, "wash-dev-component").unwrap();
        assert_eq!(dev.local_resources.environment.get("LOG").unwrap(), "debug");
        assert_eq!(dev.local_resources.config.get("flag").unwrap(), "on");
        assert_eq!(
            &dev.local_resources.allowed_hosts[..],
            &["https://api.example.com".parse().unwrap()]
        );

        let sidecar = find_component(&workload, "sidecar-a").unwrap();
        assert_eq!(
            sidecar.local_resources.environment.get("LOG").unwrap(),
            "debug"
        );
        assert_eq!(sidecar.local_resources.config.get("flag").unwrap(), "on");
        assert_eq!(
            &sidecar.local_resources.allowed_hosts[..],
            &["https://api.example.com".parse().unwrap()]
        );
    }

    #[test]
    fn build_workload_sidecar_carries_its_own_merged_values() {
        // A sidecar's pre-merged per-component values land on that sidecar's
        // LocalResources; the dev component keeps the workload-level values.
        let resolved = ResolvedWorkload {
            environment: HashMap::from([("LOG".into(), "debug".into())]),
            allowed_hosts: vec![wash_runtime::host::allowed_hosts::AllowedHost::Any],
            ..Default::default()
        };
        let sidecar_resolved = ResolvedWorkload {
            environment: HashMap::from([
                ("LOG".into(), "trace".into()),
                ("SIDECAR_ONLY".into(), "1".into()),
            ]),
            config: HashMap::from([("flag".into(), "on".into())]),
            allowed_hosts: vec!["https://api.example.com".parse().unwrap()],
        };
        let dev_cfg = DevConfig {
            components: vec![dev_component_named("sidecar-a")],
            ..Default::default()
        };
        let sidecars = vec![loaded_sidecar("sidecar-a", sidecar_resolved)];

        let workload = build_workload(
            &dev_cfg,
            fake_bytes("dev"),
            HashSet::new(),
            sidecars,
            None,
            None,
            &resolved,
        );

        let dev = find_component(&workload, "wash-dev-component").unwrap();
        assert_eq!(dev.local_resources.environment.get("LOG").unwrap(), "debug");
        assert!(!dev.local_resources.environment.contains_key("SIDECAR_ONLY"));
        assert!(dev.local_resources.config.is_empty());

        let sidecar = find_component(&workload, "sidecar-a").unwrap();
        assert_eq!(
            sidecar.local_resources.environment.get("LOG").unwrap(),
            "trace"
        );
        assert_eq!(
            sidecar
                .local_resources
                .environment
                .get("SIDECAR_ONLY")
                .unwrap(),
            "1"
        );
        assert_eq!(sidecar.local_resources.config.get("flag").unwrap(), "on");
        assert_eq!(
            &sidecar.local_resources.allowed_hosts[..],
            &["https://api.example.com".parse().unwrap()]
        );
    }

    #[test]
    fn build_workload_service_mode_lands_workload_values_on_service() {
        // When dev.service = true the dev bytes become the workload's
        // Service, and the workload values land there instead of on a
        // component.
        let resolved = ResolvedWorkload {
            environment: HashMap::from([("LOG".into(), "trace".into())]),
            ..Default::default()
        };
        let dev_cfg = DevConfig {
            service: true,
            ..Default::default()
        };

        let workload = build_workload(
            &dev_cfg,
            fake_bytes("dev"),
            HashSet::new(),
            Vec::new(),
            None,
            None,
            &resolved,
        );

        let svc = workload
            .service
            .as_ref()
            .expect("service mode should produce a Service");
        assert_eq!(svc.local_resources.environment.get("LOG").unwrap(), "trace");
        // No "wash-dev-component" Component when running as service.
        assert!(find_component(&workload, "wash-dev-component").is_none());
    }

    #[test]
    fn build_workload_service_file_sidecar_gets_workload_values() {
        // dev.service = false + dev.service_file = Some(...) loads a sidecar
        // service alongside the dev component. Workload values are
        // workload-wide, so the sidecar service receives them too.
        let resolved = ResolvedWorkload {
            environment: HashMap::from([("LOG".into(), "info".into())]),
            ..Default::default()
        };
        let dev_cfg = DevConfig::default();

        let workload = build_workload(
            &dev_cfg,
            fake_bytes("dev"),
            HashSet::new(),
            Vec::new(),
            Some(fake_bytes("svc-sidecar")),
            None,
            &resolved,
        );

        let svc = workload
            .service
            .as_ref()
            .expect("service_file should produce a Service");
        assert_eq!(svc.local_resources.environment.get("LOG").unwrap(), "info");
        let dev = find_component(&workload, "wash-dev-component").unwrap();
        assert_eq!(dev.local_resources.environment.get("LOG").unwrap(), "info");
    }

    #[test]
    fn build_workload_volumes_mount_into_every_component_and_service() {
        // dev.volumes get mounted into every Component and the Service, with
        // matching VolumeMount entries pointing at the workload-level Volume.
        let dev_cfg = DevConfig {
            volumes: vec![DevVolume {
                host_path: PathBuf::from("/host"),
                guest_path: PathBuf::from("/guest"),
            }],
            components: vec![dev_component_named("sidecar")],
            ..Default::default()
        };
        let sidecars = vec![loaded_sidecar("sidecar", ResolvedWorkload::default())];

        let workload = build_workload(
            &dev_cfg,
            fake_bytes("dev"),
            HashSet::new(),
            sidecars,
            Some(fake_bytes("svc")),
            None,
            &ResolvedWorkload::default(),
        );

        assert_eq!(workload.volumes.len(), 1);
        let volume_name = &workload.volumes[0].name;

        for c in &workload.components {
            assert_eq!(
                c.local_resources.volume_mounts.len(),
                1,
                "component {} missing mount",
                c.name
            );
            assert_eq!(&c.local_resources.volume_mounts[0].name, volume_name);
            assert_eq!(c.local_resources.volume_mounts[0].mount_path, "/guest");
        }
        let svc = workload.service.unwrap();
        assert_eq!(svc.local_resources.volume_mounts.len(), 1);
        assert_eq!(&svc.local_resources.volume_mounts[0].name, volume_name);
    }

    #[test]
    fn build_workload_sidecar_only_wasi_config_importer_triggers_injection() {
        // The dev component imports nothing wasi:config-related; a sidecar
        // does. End-to-end: the workload's host_interfaces should contain
        // a wasi:config entry populated with workload.config.
        let resolved = ResolvedWorkload {
            config: HashMap::from([("KEY".into(), "value".into())]),
            ..Default::default()
        };
        let dev_cfg = DevConfig {
            components: vec![dev_component_named("sidecar")],
            ..Default::default()
        };
        let sidecars = vec![LoadedComponent {
            name: "sidecar".into(),
            bytes: fake_bytes("sidecar"),
            interfaces: HashSet::from([iface("wasi", "config")]),
            workload: ResolvedWorkload::default(),
        }];

        let workload = build_workload(
            &dev_cfg,
            fake_bytes("dev"),
            HashSet::from([iface("wasi", "http")]),
            sidecars,
            None,
            None,
            &resolved,
        );

        let entry = find_iface(&workload.host_interfaces, "wasi", "config")
            .expect("sidecar import should have triggered wasi:config injection");
        assert_eq!(entry.config.get("KEY").unwrap(), "value");
    }

    #[test]
    fn build_workload_service_file_interfaces_reach_host_interfaces() {
        // Regression for #5351: a service_file that imports a plugin
        // interface (e.g. wasi:keyvalue) must have that interface folded into
        // the workload's host_interfaces so the plugin binds to the service.
        // Here neither the dev component nor any sidecar imports it, so the
        // interface can only reach host_interfaces via the service file.
        let dev_cfg = DevConfig::default();

        let workload = build_workload(
            &dev_cfg,
            fake_bytes("dev"),
            HashSet::new(),
            Vec::new(),
            Some(fake_bytes("svc")),
            Some(HashSet::from([iface("wasi", "keyvalue")])),
            &ResolvedWorkload::default(),
        );

        assert!(
            find_iface(&workload.host_interfaces, "wasi", "keyvalue").is_some(),
            "service file import should appear in host_interfaces"
        );
    }

    #[test]
    fn build_workload_assembles_components_in_dev_then_sidecar_order() {
        // Sanity: the dev component is component[0]; sidecars follow in
        // dev_config.components order. Some downstream wiring depends on
        // the dev component being identifiable as "wash-dev-component" but
        // it's worth pinning the order too.
        let dev_cfg = DevConfig {
            components: vec![
                dev_component_named("first-sidecar"),
                dev_component_named("second-sidecar"),
            ],
            ..Default::default()
        };
        let sidecars = vec![
            loaded_sidecar("first-sidecar", ResolvedWorkload::default()),
            loaded_sidecar("second-sidecar", ResolvedWorkload::default()),
        ];

        let workload = build_workload(
            &dev_cfg,
            fake_bytes("dev"),
            HashSet::new(),
            sidecars,
            None,
            None,
            &ResolvedWorkload::default(),
        );

        let names: Vec<_> = workload
            .components
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["wash-dev-component", "first-sidecar", "second-sidecar"]
        );
    }

    // ---------------------------------------------------------------------
    // Tests for `build_workload_host_interfaces`.
    // These cover edge cases of the merge/inject logic in isolation; the
    // build_workload tests above prove the helper is actually invoked.
    // ---------------------------------------------------------------------

    #[test]
    fn multi_component_workload_shares_one_wasi_config_entry() {
        // Two components both import wasi:config. The workload-level
        // host_interfaces list should contain exactly one wasi:config entry,
        // populated from workload.config — both components bind the same map.
        let comp_a = HashSet::from([iface("wasi", "config"), iface("wasi", "http")]);
        let comp_b = HashSet::from([iface("wasi", "config"), iface("wasi", "logging")]);
        let workload_cfg = HashMap::from([
            ("feature.flags".into(), "v2,beta".into()),
            ("LOG_LEVEL".into(), "debug".into()),
        ]);

        let result = build_workload_host_interfaces(Vec::new(), &[comp_a, comp_b], &workload_cfg);

        let wasi_config: Vec<_> = result
            .iter()
            .filter(|i| i.namespace == "wasi" && i.package == "config")
            .collect();
        assert_eq!(
            wasi_config.len(),
            1,
            "expected a single wasi:config entry shared across both components, got {}",
            wasi_config.len()
        );
        assert_eq!(
            wasi_config[0].config.get("feature.flags").unwrap(),
            "v2,beta"
        );
        assert_eq!(wasi_config[0].config.get("LOG_LEVEL").unwrap(), "debug");

        // The other interfaces from each component should also appear.
        assert!(find_iface(&result, "wasi", "http").is_some());
        assert!(find_iface(&result, "wasi", "logging").is_some());
    }

    #[test]
    fn multi_component_workload_only_one_imports_wasi_config() {
        // Only the second component imports wasi:config. The injection
        // should still happen — workload.config is workload-scoped, so a
        // sidecar-only importer is enough to surface it. The non-importing
        // component is unaffected.
        let comp_no_config = HashSet::from([iface("wasi", "http"), iface("wasi", "cli")]);
        let comp_with_config = HashSet::from([iface("wasi", "config"), iface("wasi", "logging")]);
        let workload_cfg = HashMap::from([("KEY".into(), "value".into())]);

        let result = build_workload_host_interfaces(
            Vec::new(),
            &[comp_no_config, comp_with_config],
            &workload_cfg,
        );

        let wasi_config = find_iface(&result, "wasi", "config")
            .expect("wasi:config entry should be present because one component imports it");
        assert_eq!(wasi_config.config.get("KEY").unwrap(), "value");

        // Non-importer's interfaces are still merged in.
        assert!(find_iface(&result, "wasi", "http").is_some());
        assert!(find_iface(&result, "wasi", "cli").is_some());
    }

    #[test]
    fn no_component_imports_wasi_config_means_no_injection() {
        // Even with non-empty workload.config, if no component imports
        // wasi:config we don't add a dead entry — leaves room for a future
        // explicit declaration without surprise overrides.
        let comp = HashSet::from([iface("wasi", "http")]);
        let workload_cfg = HashMap::from([("KEY".into(), "value".into())]);

        let result = build_workload_host_interfaces(Vec::new(), &[comp], &workload_cfg);

        assert!(find_iface(&result, "wasi", "config").is_none());
    }

    #[test]
    fn empty_workload_config_means_no_injection_even_when_imported() {
        // The component imports wasi:config so the entry gets merged in
        // either way. The injection contract is that no *workload* values
        // are added — verify the entry's config map stays empty.
        let comp = HashSet::from([iface("wasi", "config")]);
        let result = build_workload_host_interfaces(Vec::new(), &[comp], &HashMap::new());
        let entry = find_iface(&result, "wasi", "config").unwrap();
        assert!(
            entry.config.is_empty(),
            "no workload.config means no injected values"
        );
    }

    #[test]
    fn explicit_dev_host_interfaces_wasi_config_wins_on_conflicts() {
        // User declared wasi:config in dev.host_interfaces with their own
        // values for some keys. Workload.config fills in missing keys but
        // does not overwrite user-declared ones.
        let user_declared = iface_with_config(
            "wasi",
            "config",
            &[("LOG_LEVEL", "user_value"), ("USER_ONLY", "yes")],
        );
        let comp = HashSet::from([iface("wasi", "config")]);
        let workload_cfg = HashMap::from([
            ("LOG_LEVEL".into(), "workload_value".into()),
            ("WORKLOAD_ONLY".into(), "yes".into()),
        ]);

        let result = build_workload_host_interfaces(vec![user_declared], &[comp], &workload_cfg);

        let entry = find_iface(&result, "wasi", "config").unwrap();
        assert_eq!(
            entry.config.get("LOG_LEVEL").unwrap(),
            "user_value",
            "explicit dev.host_interfaces declaration must win on conflict"
        );
        assert_eq!(entry.config.get("USER_ONLY").unwrap(), "yes");
        assert_eq!(
            entry.config.get("WORKLOAD_ONLY").unwrap(),
            "yes",
            "workload.config keys not in user declaration must fill in"
        );
    }

    #[test]
    fn user_declared_wasi_config_untouched_when_no_component_imports_it() {
        // User declared wasi:config in dev.host_interfaces with their own
        // values, but nothing in the workload imports wasi:config. The
        // injection branch is gated on at least one importer, so the user's
        // entry must pass through unmodified. The workload.config doesn't
        // sneak values in via a non-importer.
        let user_declared = iface_with_config("wasi", "config", &[("USER_KEY", "user_value")]);
        let comp = HashSet::from([iface("wasi", "http")]);
        let workload_cfg = HashMap::from([("WORKLOAD_KEY".into(), "workload_value".into())]);

        let result = build_workload_host_interfaces(vec![user_declared], &[comp], &workload_cfg);

        let entry = find_iface(&result, "wasi", "config").unwrap();
        assert_eq!(
            entry.config.len(),
            1,
            "no workload values should have been merged in"
        );
        assert_eq!(entry.config.get("USER_KEY").unwrap(), "user_value");
        assert!(!entry.config.contains_key("WORKLOAD_KEY"));
    }

    #[test]
    fn merging_does_not_duplicate_interfaces() {
        // Two components import the same wasi:http; only one entry should
        // appear. Important when checking shared config as duplicates would
        // confuse plugin binding.
        let comp_a = HashSet::from([iface("wasi", "http")]);
        let comp_b = HashSet::from([iface("wasi", "http"), iface("wasi", "logging")]);
        let result = build_workload_host_interfaces(Vec::new(), &[comp_a, comp_b], &HashMap::new());

        let http_count = result
            .iter()
            .filter(|i| i.namespace == "wasi" && i.package == "http")
            .count();
        assert_eq!(http_count, 1);
    }
}
