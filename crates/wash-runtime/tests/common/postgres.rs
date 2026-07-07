//! Shared helpers for the async `wasmcloud:postgres@0.2.0` integration tests: a
//! postgres container, the default host interface, admin seeding, and a host
//! running the `postgres-stream-p3` fixture.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    plugin::wasmcloud_postgres::WasmcloudPostgres,
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

use super::http_incoming_handler_interface;

/// The single async postgres p3 fixture; its query is chosen by request path.
const STREAM_FIXTURE_WASM: &[u8] = include_bytes!("../wasm/postgres_stream_p3.wasm");

/// The unnamed (default) async `wasmcloud:postgres@0.2.0` interface. `types` and
/// `query` are imported unlabeled; `database` selects the target database within
/// the plugin's bouncer connection.
pub fn async_pg_interface(database: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "postgres".to_string(),
        interfaces: ["types".to_string(), "query".to_string()]
            .into_iter()
            .collect(),
        version: Some(semver::Version::parse("0.2.0").unwrap()),
        config: HashMap::from([("database".to_string(), database.to_string())]),
        name: None,
    }
}

/// Start a postgres container; returns it (kept alive by the caller) with its
/// `host:port`.
pub async fn start_postgres() -> Result<(ContainerAsync<GenericImage>, String)> {
    let container = GenericImage::new("postgres", "16-alpine")
        .with_exposed_port(5432.tcp())
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_PASSWORD", "postgres")
        .start()
        .await
        .map_err(|e| anyhow!("failed to start postgres: {e}"))?;
    let port = container.get_host_port_ipv4(5432).await?;
    Ok((container, format!("127.0.0.1:{port}")))
}

/// Connect directly (bypassing the plugin) to run admin DDL, waiting for the
/// container to accept connections. Returns a live client; the caller keeps it
/// only long enough to seed.
pub async fn admin_client(url: &str) -> Result<tokio_postgres::Client> {
    for _ in 0..40 {
        match tokio_postgres::connect(url, tokio_postgres::NoTls).await {
            Ok((client, connection)) => {
                // Drive the connection in the background for the client's life.
                tokio::spawn(async move {
                    let _ = connection.await;
                });
                return Ok(client);
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    bail!("postgres never became ready")
}

/// Stand up a host with the postgres plugin (bouncer URL carrying credentials;
/// the database is chosen per-workload via config) plus an HTTP entrypoint, and
/// start the `postgres-stream-p3` workload under `host_header`.
pub async fn start_postgres_workload(
    host_addr: &str,
    host_header: &str,
) -> Result<(std::net::SocketAddr, impl HostApi)> {
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_server.addr();

    // Base URL without a database — the plugin strips it and the workload's
    // interface config supplies `database`.
    let bouncer_url = format!("postgres://postgres:postgres@{host_addr}/");
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(WasmcloudPostgres::new(&bouncer_url)?))?
        .build()?;
    let host = host.start().await.context("failed to start host")?;

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "postgres-stream-p3".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "postgres-stream-p3.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(STREAM_FIXTURE_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: vec![
                http_incoming_handler_interface(host_header, None),
                async_pg_interface("postgres"),
            ],
            volumes: vec![],
        },
    };

    let resp = host
        .workload_start(req)
        .await
        .context("workload_start call failed")?;
    assert_eq!(
        resp.workload_status.workload_state,
        WorkloadState::Running,
        "workload should resolve: {}",
        resp.workload_status.message
    );

    Ok((addr, host))
}
