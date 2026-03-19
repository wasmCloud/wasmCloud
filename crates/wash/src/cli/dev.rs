use std::{collections::HashMap, sync::Arc};

use anyhow::{Context as _, bail, ensure};
use bytes::Bytes;
use clap::Args;
use tokio::{select, sync::mpsc};
use tracing::{debug, info, instrument, warn};
use wash_runtime::{
    engine::Engine,
    host::{Host, HostApi},
    observability::Meters,
    plugin::{self},
    types::{
        Component, HostPathVolume, LocalResources, Service, Volume, VolumeMount, VolumeType,
        Workload, WorkloadStartRequest, WorkloadState, WorkloadStopRequest,
    },
};

use crate::{
    cli::{CliCommand, CliContext, CommandOutput, component_build::build_dev_component},
    config::{Config, load_config},
    wit::WitConfig,
};

/// Start a development server for a Wasm component
#[derive(Debug, Clone, Args)]
pub struct DevCommand {}

impl CliCommand for DevCommand {
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
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

        let engine = Engine::builder()
            .with_pooling_allocator(true)
            .with_fuel_consumption(ctx.enable_meters())
            .build()?;

        let mut host_builder = Host::builder()
            .with_engine(engine)
            .with_meters(Meters::new(ctx.enable_meters()));

        // Enable wasi config
        host_builder =
            host_builder.with_plugin(Arc::new(plugin::wasi_config::DynamicConfig::default()))?;

        // Enable wasmcloud:messaging
        host_builder = host_builder.with_plugin(Arc::new(
            plugin::wasmcloud_messaging::InMemoryMessaging::default(),
        ))?;

        // Add blobstore plugin
        if let Some(blobstore_path) = &dev_config.wasi_blobstore_path {
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasi_blobstore::FilesystemBlobstore::new(blobstore_path.clone()),
            ))?;
            debug!(
                path = %blobstore_path.display(),
                "WASI Blobstore plugin registered with filesystem backend"
            );
        } else {
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasi_blobstore::InMemoryBlobstore::default(),
            ))?;
            debug!("WASI Blobstore plugin registered with in-memory backend");
        }

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

            let http_server = wash_runtime::host::http::HttpServer::new_with_tls(
                http_handler,
                http_addr.parse()?,
                cert_path,
                key_path,
                dev_config.tls_ca_path.as_deref(),
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

        // Add keyvalue plugin — Redis > NATS > filesystem > in-memory
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
        } else {
            host_builder = host_builder
                .with_plugin(Arc::new(plugin::wasi_keyvalue::InMemoryKeyValue::default()))?;
            debug!("WASI KeyValue plugin registered with in-memory backend");
        }

        // Add postgres plugin if configured
        if let Some(postgres_url) = &dev_config.postgres_url {
            host_builder = host_builder.with_plugin(Arc::new(
                plugin::wasmcloud_postgres::WasmcloudPostgres::new(postgres_url)
                    .context("failed to configure postgres plugin")?,
            ))?;
            debug!("wasmcloud:postgres plugin registered");
        }

        // Add otel plugin
        if dev_config.wasi_otel {
            host_builder =
                host_builder.with_plugin(Arc::new(plugin::wasi_otel::WasiOtel::default()))?;
            debug!("WASI OpenTelemetry plugin registered");
        }

        // Enable WASI WebGPU if requested
        #[cfg(not(target_os = "windows"))]
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
        let workload = create_workload(&host, &config, wasm_bytes.into()).await?;
        // Running workload ID for reloads
        let workload_id = reload_component(host.clone(), &workload, None).await?;

        info!(address = %format!("{}://{}", protocol, http_addr), "listening for HTTP requests");

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

/// Create the [`Workload`] structure for the development component
///
/// ## Arguments
/// - `config`: The overall Wash configuration
/// - `bytes`: The bytes of the component under development
async fn create_workload(host: &Host, config: &Config, bytes: Bytes) -> anyhow::Result<Workload> {
    let dev_config = config.dev();

    let mut volumes = Vec::<Volume>::new();
    let mut volume_mounts = Vec::<VolumeMount>::new();

    dev_config.volumes.iter().for_each(|cfg_volume| {
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
    });

    // Extract both imports and exports from the component
    // This populates host_interfaces which is checked bidirectionally during plugin binding
    let mut host_interfaces = dev_config.host_interfaces.clone();

    let mut service: Option<Service> = None;
    let mut components = Vec::new();
    if dev_config.service {
        service = Some(Service {
            bytes,
            digest: None,
            max_restarts: 0,
            local_resources: LocalResources {
                volume_mounts: volume_mounts.clone(),
                ..Default::default()
            },
        })
    } else {
        let component_interfaces = host
            .intersect_interfaces(&bytes)
            .context("failed to extract component interfaces")?;

        // Merge component interfaces into host_interfaces
        for interface in component_interfaces {
            match host_interfaces
                .iter()
                .find(|i| i.namespace == interface.namespace && i.package == interface.package)
            {
                Some(_) => {}
                None => host_interfaces.push(interface),
            }
        }

        components.push(Component {
            name: "wash-dev-component".to_string(),
            bytes,
            digest: None,
            local_resources: LocalResources {
                volume_mounts: volume_mounts.clone(),
                ..Default::default()
            },
            pool_size: -1,
            max_invocations: -1,
        });

        if let Some(service_path) = &dev_config.service_file {
            let service_bytes = tokio::fs::read(service_path).await.with_context(|| {
                format!("failed to read service file at {}", service_path.display())
            })?;

            service = Some(Service {
                bytes: Bytes::from(service_bytes),
                digest: None,
                max_restarts: 0,
                local_resources: LocalResources {
                    volume_mounts: volume_mounts.clone(),
                    ..Default::default()
                },
            });
        }
    }

    for dev_component in &dev_config.components {
        let comp_bytes = tokio::fs::read(&dev_component.file)
            .await
            .with_context(|| {
                format!(
                    "failed to read component file at {}",
                    dev_component.file.display()
                )
            })?;

        let comp_interfaces = host
            .intersect_interfaces(&comp_bytes)
            .context("failed to extract component interfaces")?;

        // Merge component interfaces into host_interfaces
        for interface in comp_interfaces {
            match host_interfaces
                .iter()
                .find(|i| i.namespace == interface.namespace && i.package == interface.package)
            {
                Some(_) => {}
                None => host_interfaces.push(interface),
            }
        }

        components.push(Component {
            name: dev_component.name.clone(),
            bytes: Bytes::from(comp_bytes),
            digest: None,
            local_resources: LocalResources {
                volume_mounts: volume_mounts.clone(),
                ..Default::default()
            },
            pool_size: -1,
            max_invocations: -1,
        });
    }

    debug!("workload host interfaces: {:?}", host_interfaces);

    Ok(Workload {
        namespace: "default".to_string(),
        name: "dev".to_string(),
        annotations: HashMap::default(),
        components,
        host_interfaces,
        service,
        volumes,
    })
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
