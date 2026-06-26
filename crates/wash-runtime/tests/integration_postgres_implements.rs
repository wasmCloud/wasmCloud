//! Full guest-driven e2e for `(implements ..)` named-import postgres routing.
//!
//! The `postgres-implements` fixture imports `wasmcloud:postgres/query` **twice**
//! under the component-model labels `team-a` and `team-b` (the WIT named-import
//! clause — `import team-a: wasmcloud:postgres/query@…;`). On each HTTP request
//! it reads team A's table through both labels and answers `isolated` only when
//! the `team-a` read returns a row and the `team-b` read is refused.
//!
//! The host binds a single [`WasmcloudPostgres`] plugin and declares the two
//! named interfaces, each carrying a different postgres role's credentials. The
//! plugin's named-imports `add_to_linker` resolves each label to its own
//! credentialed connection pool; postgres' RBAC then enforces that `team_b`
//! (no grant) cannot read `team_a`'s table. Success proves the implements id
//! threads from the guest import label all the way to the right connection.
//!
//! Requires Docker; marked `#[ignore]`, so it runs only under
//! `cargo test --include-ignored` (CI's Linux leg) and not a plain `cargo test`.
#![cfg(all(
    feature = "wasmcloud-postgres",
    feature = "wasm_component_model_implements"
))]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashMap, HashSet};
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
    plugin::wasmcloud_postgres::{PgId, WasmcloudPostgres},
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

mod common;
use common::http_incoming_handler_interface;

const POSTGRES_IMPLEMENTS_WASM: &[u8] = include_bytes!("wasm/postgres_implements.wasm");

const PG_VERSION: &str = "0.1.1-draft";

/// A named `wasmcloud:postgres/query` interface routed to one role's connection.
/// The `name` becomes the implements label the guest imports under (`team-a` /
/// `team-b`); the host resolves it against the component's import label, and the
/// `url` carries that role's credentials and database.
fn named_pg(name: &str, url: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "postgres".to_string(),
        interfaces: ["query".to_string()].into_iter().collect(),
        version: Some(semver::Version::parse(PG_VERSION).unwrap()),
        config: HashMap::from([("url".to_string(), url.to_string())]),
        name: Some(name.to_string()),
    }
}

/// Build a single connection pool from one URL (used for admin DB setup).
async fn connect(url: &str) -> Result<PgId> {
    let registry = WasmcloudPostgres::multiplexer()
        .build_registry(&HashSet::from([named_pg("admin", url)]))
        .await?;
    registry.get("admin").cloned().context("admin routed")
}

/// Wait for postgres to accept connections through the given pool.
async fn wait_ready(id: &PgId) -> Result<()> {
    for _ in 0..40 {
        if id.execute("SELECT 1").await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bail!("postgres never became ready")
}

#[tokio::test]
#[ignore = "requires Docker (postgres); run with `cargo test --include-ignored`"]
async fn implements_imports_route_to_per_credential_connections() -> Result<()> {
    // One postgres instance.
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

    let admin_url = format!("postgres://postgres:postgres@{host_addr}/postgres");
    let team_a_url = format!("postgres://team_a:pw_a@{host_addr}/postgres");
    let team_b_url = format!("postgres://team_b:pw_b@{host_addr}/postgres");

    // As superuser: two login roles, a table owned by team_a with one row, and a
    // grant only to team_a. team_b is deliberately left with no access.
    let admin = connect(&admin_url).await?;
    wait_ready(&admin).await?;
    for stmt in [
        "CREATE ROLE team_a LOGIN PASSWORD 'pw_a'",
        "CREATE ROLE team_b LOGIN PASSWORD 'pw_b'",
        "CREATE TABLE team_a_data (id INT PRIMARY KEY, val TEXT)",
        "INSERT INTO team_a_data (id, val) VALUES (1, 'hello')",
        "GRANT ALL ON team_a_data TO team_a",
    ] {
        admin
            .execute(stmt)
            .await
            .map_err(|e| anyhow!("admin setup failed on `{stmt}`: {e:?}"))?;
    }

    // Stand up a host with the postgres plugin (no shared bouncer URL: purely
    // implements-routed) plus an HTTP entrypoint to drive the guest.
    let engine = Engine::builder().build()?;
    let http_server = HttpServer::new(DevRouter::default(), "127.0.0.1:0".parse()?).await?;
    let addr = http_server.addr();

    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_server))
        .with_plugin(Arc::new(WasmcloudPostgres::multiplex_only()))?
        .build()?;
    let host = host.start().await.context("failed to start host")?;

    // The two named interfaces map to the guest's `team-a` / `team-b` query
    // imports; each routes to a connection with that role's credentials.
    let host_interfaces = vec![
        http_incoming_handler_interface("pg-implements", None),
        named_pg("team-a", &team_a_url),
        named_pg("team-b", &team_b_url),
    ];

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "postgres-implements".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: "postgres-implements.wasm".to_string(),
                digest: None,
                bytes: bytes::Bytes::from_static(POSTGRES_IMPLEMENTS_WASM),
                local_resources: LocalResources::default(),
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces,
            volumes: vec![],
        },
    };

    // Binding runs the named-imports `add_to_linker`, resolving `team-a`/`team-b`
    // against the declared interfaces. `workload_start` returns Ok even on
    // resolution failure (it encodes the error in the status), so assert the
    // workload actually reached Running.
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

    // Drive the guest: it reads team_a's table through both labels. `isolated`
    // means team-a (owner) read its row while team-b (no grant) was refused —
    // i.e. each implements import resolved to its own credentialed connection.
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "pg-implements")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    let status = response.status();
    let body = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {body}");
    assert_eq!(
        body, "isolated",
        "each implements import must route to its own credentialed connection \
         (team-a reads, team-b denied)"
    );

    Ok(())
}
