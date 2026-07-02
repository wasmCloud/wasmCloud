//! End-to-end for the async (wasip3) `wasmcloud:postgres@0.2.0` surface.
//!
//! The `postgres-async-p3` fixture imports the unnamed (default) `query`
//! interface and exports a p3 `wasi:http/handler`. On
//! each request it runs `SELECT val FROM items ORDER BY id`, drains the returned
//! `stream<row>` one row at a time, then awaits the completion `future`, and
//! writes back the column list and streamed values.
//!
//! The host binds a single [`WasmcloudPostgres`] with a base bouncer URL; the
//! workload's unnamed `wasmcloud:postgres@0.2.0` interface supplies the target
//! database via config. Success (`cols=val rows=alpha,beta,gamma`) proves the
//! full async path: the host's `store`-based `query` binding, incremental row
//! streaming through the bounded channel, and the completion future.
//!
//! Requires Docker; marked `#[ignore]`, so it runs only under
//! `cargo test --include-ignored` (CI's Linux leg) and not a plain `cargo test`.
#![cfg(feature = "wasmcloud-postgres")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::time::timeout;

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

mod common;
use common::http_incoming_handler_interface;

const POSTGRES_ASYNC_P3_WASM: &[u8] = include_bytes!("wasm/postgres_async_p3.wasm");

/// The unnamed (default) async `wasmcloud:postgres@0.2.0` interface. `types` and
/// `query` are imported unlabeled; `database` selects the target database within
/// the plugin's bouncer connection.
fn async_pg_interface(database: &str) -> WitInterface {
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

/// Connect directly (bypassing the plugin) to run admin DDL, waiting for the
/// container to accept connections. Returns a live client; the caller keeps it
/// only long enough to seed the table.
async fn admin_client(url: &str) -> Result<tokio_postgres::Client> {
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

#[tokio::test]
#[ignore = "requires Docker (postgres); run with `cargo test --include-ignored`"]
async fn async_query_streams_rows_to_a_default_import() -> Result<()> {
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
    let host_addr = format!("127.0.0.1:{port}");

    // Seed a table with a few ordered rows the guest will stream back.
    let admin_url = format!("postgres://postgres:postgres@{host_addr}/postgres");
    let admin = admin_client(&admin_url).await?;
    admin
        .batch_execute(
            "CREATE TABLE items (id INT PRIMARY KEY, val TEXT);
             INSERT INTO items (id, val) VALUES (1, 'alpha'), (2, 'beta'), (3, 'gamma');",
        )
        .await
        .context("failed to seed items table")?;

    // Host with the postgres plugin (bouncer URL carries credentials; the
    // database is chosen per-workload via config) plus an HTTP entrypoint.
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
            name: "postgres-async-p3".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "postgres-async-p3.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(POSTGRES_ASYNC_P3_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: vec![
                http_incoming_handler_interface("pg-async", None),
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

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "pg-async")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    let status = response.status();
    let body = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {body}");
    assert_eq!(
        body, "cols=val rows=alpha,beta,gamma",
        "the guest should stream every row of the async query result back in order"
    );

    Ok(())
}
