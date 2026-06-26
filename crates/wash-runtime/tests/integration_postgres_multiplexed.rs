//! Backend-level test for per-credential `wasmcloud:postgres` routing via the
//! multiplexer (the registry/`PgId` path, no guest component — see
//! `integration_postgres_implements.rs` for the full guest-driven e2e).
//!
//! ONE postgres instance, TWO database roles with different privileges: `team_a`
//! owns a table, `team_b` has no grant on it. Two named host interfaces
//! (`team-a`, `team-b`) carry each role's credentials; the multiplexer builds an
//! isolated connection pool per import. We assert team B — routed through its
//! own credentials — is denied read/write on team A's table, while team A
//! succeeds. Postgres' own RBAC enforces the isolation.
//!
//! Requires Docker; marked `#[ignore]`, so it runs only under
//! `cargo test --include-ignored` (CI's Linux leg) and not a plain `cargo test`.
#![cfg(all(
    feature = "wasmcloud-postgres",
    feature = "wasm_component_model_implements"
))]

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use testcontainers::{
    GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};

use wash_runtime::plugin::wasmcloud_postgres::{PgId, WasmcloudPostgres};
use wash_runtime::wit::WitInterface;

/// A named `wasmcloud:postgres` host interface carrying one role's full URL.
fn pg_iface(name: &str, url: &str) -> WitInterface {
    WitInterface {
        namespace: "wasmcloud".to_string(),
        package: "postgres".to_string(),
        interfaces: [
            "query".to_string(),
            "prepared".to_string(),
            "types".to_string(),
        ]
        .into_iter()
        .collect(),
        version: None,
        config: HashMap::from([("url".to_string(), url.to_string())]),
        name: Some(name.to_string()),
    }
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
async fn postgres_implements_enforces_per_team_credentials() -> Result<()> {
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
    let host = format!("127.0.0.1:{port}");

    let admin_url = format!("postgres://postgres:postgres@{host}/postgres");
    let team_a_url = format!("postgres://team_a:pw_a@{host}/postgres");
    let team_b_url = format!("postgres://team_b:pw_b@{host}/postgres");

    // Build a connection pool per named import through the multiplexer — the
    // per-credential routing under test.
    let interfaces = HashSet::from([
        pg_iface("admin", &admin_url),
        pg_iface("team-a", &team_a_url),
        pg_iface("team-b", &team_b_url),
    ]);
    let registry = WasmcloudPostgres::multiplexer()
        .build_registry(&interfaces)
        .await?;
    let admin = registry.get("admin").expect("admin routed");
    let team_a = registry.get("team-a").expect("team-a routed");
    let team_b = registry.get("team-b").expect("team-b routed");

    wait_ready(admin).await?;

    // As superuser: two roles, a table owned by team_a, grant only to team_a.
    for stmt in [
        "CREATE ROLE team_a LOGIN PASSWORD 'pw_a'",
        "CREATE ROLE team_b LOGIN PASSWORD 'pw_b'",
        "CREATE TABLE team_a_data (id INT PRIMARY KEY, val TEXT)",
        "GRANT ALL ON team_a_data TO team_a",
    ] {
        admin
            .execute(stmt)
            .await
            .map_err(|e| anyhow!("admin setup failed on `{stmt}`: {e:?}"))?;
    }

    // Team A (its own credentials) can write and read its table.
    team_a
        .execute("INSERT INTO team_a_data (id, val) VALUES (1, 'hello')")
        .await
        .map_err(|e| anyhow!("team_a insert failed: {e:?}"))?;
    let rows = team_a
        .query("SELECT val FROM team_a_data WHERE id = 1", &[])
        .await
        .map_err(|e| anyhow!("team_a read failed: {e:?}"))?;
    assert_eq!(rows.len(), 1, "team_a should read its own row");

    // Team B (routed through its own, unprivileged credentials) is denied.
    let read = team_b.query("SELECT * FROM team_a_data", &[]).await;
    let read_err = format!("{:?}", read.expect_err("team_b read should be denied"));
    assert!(
        read_err.contains("permission denied"),
        "expected permission denied, got: {read_err}"
    );

    let write = team_b
        .execute("INSERT INTO team_a_data (id, val) VALUES (2, 'nope')")
        .await;
    let write_err = format!("{:?}", write.expect_err("team_b write should be denied"));
    assert!(
        write_err.contains("permission denied"),
        "expected permission denied, got: {write_err}"
    );

    Ok(())
}
