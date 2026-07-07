//! Basic default-import e2e for the async (wasip3) `wasmcloud:postgres@0.2.0`
//! surface.
//!
//! The `postgres-stream-p3` fixture imports the unnamed (default) `query`
//! interface — no `(implements ..)` — and exports a p3 `wasi:http/handler`. Its
//! default `/` route runs `SELECT val FROM items ORDER BY id`, drains the
//! returned `stream<row>`, and forwards each value to the response body. Success
//! (`alpha\nbeta\ngamma\n`) proves the full async path — the host's `store`-based
//! `query` binding, row streaming, and the completion `future` — reaching a
//! component that never uses `implements`.
//!
//! Requires Docker; marked `#[ignore]`, so it runs only under
//! `cargo test --include-ignored` (CI's Linux leg) and not a plain `cargo test`.
#![cfg(feature = "wasmcloud-postgres")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use anyhow::{Context, Result};
use tokio::time::timeout;

mod common;
use common::postgres::{admin_client, start_postgres, start_postgres_workload};

#[tokio::test]
#[ignore = "requires Docker (postgres); run with `cargo test --include-ignored`"]
async fn async_query_streams_rows_to_a_default_import() -> Result<()> {
    let (_container, host_addr) = start_postgres().await?;

    // Seed a small ordered table the guest will stream back.
    let admin = admin_client(&format!(
        "postgres://postgres:postgres@{host_addr}/postgres"
    ))
    .await?;
    admin
        .batch_execute(
            "CREATE TABLE items (id INT PRIMARY KEY, val TEXT);
             INSERT INTO items (id, val) VALUES (1, 'alpha'), (2, 'beta'), (3, 'gamma');",
        )
        .await
        .context("failed to seed items table")?;

    let (addr, _host) = start_postgres_workload(&host_addr, "pg-basic").await?;

    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "pg-basic")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("request failed")?;

    let status = response.status();
    let body = response.text().await?;
    assert!(status.is_success(), "expected 200, got {status}: {body}");
    assert_eq!(
        body, "alpha\nbeta\ngamma\n",
        "the default import should stream every row of the query result back in order"
    );

    Ok(())
}
